import http from "node:http";
import { chromium, firefox, webkit } from "playwright";

const PORT = Number(process.env.PORT || 3001);
const HEADLESS = ["1", "true", "yes"].includes(String(process.env.HEADLESS || "0").toLowerCase());
const BROWSER_NAME = (process.env.PLAYWRIGHT_BROWSER || "chromium").toLowerCase();
const UA =
  process.env.CHROME_UA ||
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const engines = { chromium, firefox, webkit };
const engine = engines[BROWSER_NAME];
if (!engine) {
  console.error(`unknown PLAYWRIGHT_BROWSER=${BROWSER_NAME}`);
  process.exit(1);
}

let browser;

async function getBrowser() {
  if (browser && browser.isConnected()) return browser;
  browser = await engine.launch({
    headless: HEADLESS,
    args: ["--no-sandbox", "--disable-dev-shm-usage", "--disable-blink-features=AutomationControlled"],
  });
  return browser;
}

function looksLikeCloudflare({ title, bodySample }) {
  const t = `${title || ""} ${bodySample || ""}`.toLowerCase();
  return (
    t.includes("just a moment") ||
    t.includes("performing security verification") ||
    t.includes("cf-browser-verification") ||
    t.includes("challenge-platform")
  );
}

async function diagnostics(page) {
  return page.evaluate(() => ({
    title: document.title || "",
    href: location.href || "",
    cards: document.querySelectorAll("div.job-listing").length,
    bodySample: (document.body && document.body.innerText ? document.body.innerText : "").slice(0, 240),
  }));
}

const STILL_LOADING = ["po ngarkohen", "loading…", "loading...", "please wait", "duke u ngarkuar"];

async function waitForSettledText(page, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  let stableSince = 0;
  while (Date.now() < deadline) {
    const sample = await page.evaluate(() =>
      (document.body && document.body.innerText ? document.body.innerText : "").slice(0, 6000)
    );
    const loading = STILL_LOADING.some((phrase) => sample.toLowerCase().includes(phrase));
    if (!loading && sample.length > 80 && sample === last) {
      if (!stableSince) stableSince = Date.now();
      if (Date.now() - stableSince >= 1200) return;
    } else {
      stableSince = 0;
      last = sample;
    }
    await new Promise((r) => setTimeout(r, 400));
  }
}

async function waitForListing(page, selector, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const present = await page.$(selector);
    if (present) {
      const diag = await diagnostics(page);
      if (!looksLikeCloudflare(diag)) return { ready: true, diag };
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  return { ready: false, diag: await diagnostics(page) };
}

async function extractCards(page, cfg) {
  return page.evaluate((c) => {
    const text = (el) => ((el && (el.innerText || el.textContent)) || "").trim();
    if (c.card) {
      return [...document.querySelectorAll(c.card)]
        .map((card) => {
          const titleEl = c.title ? card.querySelector(c.title) : null;
          const companyEl = c.company ? card.querySelector(c.company) : null;
          const locationEl = c.location ? card.querySelector(c.location) : null;
          const dateEl = c.date ? card.querySelector(c.date) : null;
          return {
            url: titleEl && titleEl.href ? titleEl.href : null,
            title: text(titleEl) || null,
            company: text(companyEl) || null,
            location: text(locationEl) || null,
            dateText: text(dateEl) || null,
          };
        })
        .filter((row) => row.url);
    }
    const sel = c.itemLink || "a[href]";
    return [...document.querySelectorAll(sel)]
      .map((a) => a.href)
      .filter(Boolean)
      .map((url) => ({ url, title: null, company: null, location: null, dateText: null }));
  }, cfg);
}

async function scrapeList(body) {
  const url = body.url;
  if (!url) throw Object.assign(new Error("url is required"), { status: 400 });

  const maxPages = Math.min(Number(body.max_pages) || 50, 50);
  const maxItems = Math.min(Number(body.max_items) || 2000, 2000);
  const timeoutMs = Number(body.timeout_ms) || 90_000;
  const waitMs = Number(body.wait_ms) || 2000;
  const waitFor = body.wait_for || body.card_selector || "body";

  const b = await getBrowser();
  const context = await b.newContext({
    userAgent: UA,
    locale: "sq-AL",
    viewport: { width: 1365, height: 900 },
  });
  await context.addInitScript(() => {
    Object.defineProperty(navigator, "webdriver", { get: () => undefined });
  });
  const page = await context.newPage();

  const seen = new Set();
  const cards = [];
  let pagesVisited = 0;
  let current = url;

  try {
    while (pagesVisited < maxPages && cards.length < maxItems) {
      pagesVisited += 1;
      await page.goto(current, { waitUntil: "domcontentloaded", timeout: timeoutMs });
      await new Promise((r) => setTimeout(r, waitMs));
      const { ready, diag } = await waitForListing(page, waitFor, timeoutMs);
      if (!ready || looksLikeCloudflare(diag)) {
        const err = new Error(
          `Cloudflare challenge blocked DOM scrape for ${current}. Diagnostics: ${JSON.stringify(diag)}`
        );
        err.status = 503;
        throw err;
      }

      const extracted = await extractCards(page, {
        card: body.card_selector || null,
        title: body.title_selector || null,
        company: body.company_selector || null,
        location: body.location_selector || null,
        date: body.date_selector || null,
        itemLink: body.item_link_selector || null,
      });

      const re = body.item_link_regex ? new RegExp(body.item_link_regex) : null;
      for (const row of extracted) {
        if (!row.url) continue;
        if (re && !re.test(row.url)) continue;
        if (seen.has(row.url)) continue;
        seen.add(row.url);
        cards.push(row);
        if (cards.length >= maxItems) break;
      }

      if (!body.next_page_selector) break;
      const next = await page.evaluate((sel) => {
        const el = document.querySelector(sel);
        if (!el) return null;
        if (el.disabled || el.getAttribute("aria-disabled") === "true") return null;
        if (el.href) return el.href;
        el.click();
        return "clicked";
      }, body.next_page_selector);

      if (!next) break;
      if (next !== "clicked" && next !== current) current = next;
      else await new Promise((r) => setTimeout(r, 1500));
    }

    return { cards, pages_visited: pagesVisited };
  } finally {
    await context.close().catch(() => {});
  }
}

const sessions = new Map();
const SESSION_TTL_MS = 10 * 60 * 1000;

function sweepSessions() {
  const now = Date.now();
  for (const [id, session] of sessions) {
    if (now - session.lastUsed < SESSION_TTL_MS) continue;
    sessions.delete(id);
    session.context.close().catch(() => {});
  }
}

function releaseSession(id) {
  const session = sessions.get(id);
  if (!session) return;
  sessions.delete(id);
  session.context.close().catch(() => {});
}

async function readPage(page) {
  return page.evaluate(() => {
    const text = (document.body && document.body.innerText ? document.body.innerText : "").slice(0, 80_000);
    const seen = new Set();
    const links = [];
    for (const a of document.querySelectorAll("a[href]")) {
      const href = a.href;
      if (!href || seen.has(href)) continue;
      if (href.startsWith("javascript:") || href.startsWith("mailto:")) continue;
      seen.add(href);
      links.push({
        text: ((a.innerText || a.textContent || "").trim()).slice(0, 160),
        href,
      });
      if (links.length >= 400) break;
    }
    const hasNext = [...document.querySelectorAll("button, a")].some((el) => {
      const aria = (el.getAttribute("aria-label") || "").toLowerCase();
      const label = (el.innerText || el.textContent || "").trim().toLowerCase();
      const disabled =
        el.disabled ||
        el.getAttribute("aria-disabled") === "true" ||
        String(el.className || "").includes("Mui-disabled");
      if (disabled) return false;
      if (aria.includes("next page") || aria.includes("faqja tjetër") || aria === "next") return true;
      return ["next", "next page", ">", "›", "»", "tjetër", "faqja tjetër"].includes(label);
    });
    return {
      final_url: location.href || "",
      title: document.title || "",
      text,
      links,
      has_next: hasNext,
    };
  });
}

async function listingPageNumber(page) {
  return page.evaluate(() => {
    const selected = document.querySelector("button[aria-current='page'], button.Mui-selected");
    if (selected && /^\d+$/.test((selected.innerText || "").trim())) {
      return Number(selected.innerText.trim());
    }
    const text = document.body && document.body.innerText ? document.body.innerText : "";
    const match = text.match(/(\d+)\s*-\s*(\d+)\s+nga\s+\d+/);
    if (!match) return 1;
    const start = Number(match[1]);
    const end = Number(match[2]);
    const size = Math.max(1, end - start + 1);
    return Math.floor((start - 1) / size) + 1;
  });
}

async function waitForPageChange(page, before) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    const now = await page.evaluate(() =>
      (document.body && document.body.innerText ? document.body.innerText : "").slice(0, 1800)
    );
    const loading = STILL_LOADING.some((phrase) => now.toLowerCase().includes(phrase));
    if (!loading && now !== before && now.length > 80) break;
    await new Promise((r) => setTimeout(r, 300));
  }
  await waitForSettledText(page, 15_000);
}

async function jumpToListingPage(page, target) {
  target = Math.max(1, Number(target) || 1);
  if (target === 1) return;
  for (let step = 0; step < 40; step++) {
    const current = await listingPageNumber(page);
    if (current === target) return;
    const before = await page.evaluate(() =>
      (document.body && document.body.innerText ? document.body.innerText : "").slice(0, 1800)
    );
    const moved = await page.evaluate(({ target, current }) => {
      const buttons = [...document.querySelectorAll("button")];
      const enabled = (el) =>
        !(el.disabled || el.getAttribute("aria-disabled") === "true" || String(el.className || "").includes("Mui-disabled"));
      const numbered = [];
      for (const el of buttons) {
        if (!enabled(el)) continue;
        const aria = el.getAttribute("aria-label") || "";
        const match = aria.match(/go to page (\d+)/i);
        const label = (el.innerText || "").trim();
        const n = match ? Number(match[1]) : /^\d+$/.test(label) ? Number(label) : null;
        if (n && n !== current) numbered.push({ n, el });
      }
      const exact = numbered.find((item) => item.n === target);
      if (exact) {
        exact.el.click();
        return exact.n;
      }
      const clickCloser = (toward, shortcut) => {
        if (shortcut && toward) {
          const viaShortcut = Math.abs(shortcut.n - target);
          const viaToward = Math.abs(toward.n - target);
          if (viaShortcut < viaToward) {
            shortcut.el.click();
            return shortcut.n;
          }
        }
        const pick = toward || shortcut;
        if (!pick) return 0;
        pick.el.click();
        return pick.n;
      };
      if (current < target) {
        const higher = numbered.filter((item) => item.n > current).sort((a, b) => a.n - b.n);
        const toward = higher.filter((item) => item.n <= target).pop() || null;
        const shortcut = higher.find((item) => item.n > target) || null;
        const picked = clickCloser(toward, shortcut);
        if (picked) return picked;
        const next = buttons.find((el) => enabled(el) && (el.getAttribute("aria-label") || "").toLowerCase().includes("next page"));
        if (next) {
          next.click();
          return current + 1;
        }
      } else {
        const lower = numbered.filter((item) => item.n < current).sort((a, b) => b.n - a.n);
        const toward = lower.filter((item) => item.n >= target).pop() || null;
        const shortcut = lower.find((item) => item.n < target) || null;
        const picked = clickCloser(toward, shortcut);
        if (picked) return picked;
        const prev = buttons.find((el) => enabled(el) && (el.getAttribute("aria-label") || "").toLowerCase().includes("previous page"));
        if (prev) {
          prev.click();
          return current - 1;
        }
      }
      return 0;
    }, { target, current });
    if (!moved) {
      throw Object.assign(new Error(`could not open listing page ${target} (stopped on page ${current})`), {
        status: 422,
      });
    }
    await waitForPageChange(page, before);
  }
  const current = await listingPageNumber(page);
  if (current !== target) {
    throw Object.assign(new Error(`could not open listing page ${target} (stopped on page ${current})`), {
      status: 422,
    });
  }
}

async function clickNextPage(page) {
  const before = await page.evaluate(() =>
    (document.body && document.body.innerText ? document.body.innerText : "").slice(0, 1800)
  );
  const clicked = await page.evaluate(() => {
    const next = [...document.querySelectorAll("button, a")].find((el) => {
      const aria = (el.getAttribute("aria-label") || "").toLowerCase();
      const label = (el.innerText || el.textContent || "").trim().toLowerCase();
      const disabled =
        el.disabled ||
        el.getAttribute("aria-disabled") === "true" ||
        String(el.className || "").includes("Mui-disabled");
      if (disabled) return false;
      if (aria.includes("next page") || aria.includes("faqja tjetër") || aria === "next") return true;
      return ["next", "next page", ">", "›", "»", "tjetër", "faqja tjetër"].includes(label);
    });
    if (!next) return false;
    next.click();
    return true;
  });
  if (!clicked) return false;
  await waitForPageChange(page, before);
  return true;
}

async function openListing(url, timeoutMs, waitMs) {
  const b = await getBrowser();
  const context = await b.newContext({
    userAgent: UA,
    locale: "sq-AL",
    viewport: { width: 1365, height: 900 },
  });
  await context.addInitScript(() => {
    Object.defineProperty(navigator, "webdriver", { get: () => undefined });
  });
  const page = await context.newPage();
  try {
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: timeoutMs });
    await new Promise((r) => setTimeout(r, waitMs));
    const { ready, diag } = await waitForListing(page, "body", timeoutMs);
    if (ready && !looksLikeCloudflare(diag)) {
      await waitForSettledText(page, Math.min(timeoutMs, 25_000));
    }
    if (!ready || looksLikeCloudflare(diag)) {
      const err = new Error(
        `Cloudflare challenge blocked page fetch for ${url}. Diagnostics: ${JSON.stringify(diag)}`
      );
      err.status = 503;
      throw err;
    }
    return { context, page };
  } catch (err) {
    await context.close().catch(() => {});
    throw err;
  }
}

async function fetchPage(body) {
  sweepSessions();
  if (body.session_id && body.release) {
    releaseSession(String(body.session_id));
    return { final_url: "", title: "", text: "", links: [], has_next: false, session_id: null };
  }

  if (body.session_id && body.next) {
    const session = sessions.get(String(body.session_id));
    if (!session) {
      throw Object.assign(new Error("listing page session expired; start again from the first page"), {
        status: 410,
      });
    }
    session.lastUsed = Date.now();
    const moved = await clickNextPage(session.page);
    const snap = await readPage(session.page);
    snap.session_id = String(body.session_id);
    if (!moved) snap.has_next = false;
    if (!snap.has_next) {
      releaseSession(String(body.session_id));
      snap.session_id = null;
    }
    return snap;
  }

  const url = body.url;
  if (!url) throw Object.assign(new Error("url is required"), { status: 400 });
  const timeoutMs = Number(body.timeout_ms) || 90_000;
  const waitMs = Number(body.wait_ms) || 2000;
  const opened = await openListing(url, timeoutMs, waitMs);
  const startPage = Math.max(1, Number(body.start_page) || 1);
  if (startPage > 1) {
    try {
      await jumpToListingPage(opened.page, startPage);
    } catch (err) {
      await opened.context.close().catch(() => {});
      throw err;
    }
  }
  const snap = await readPage(opened.page);
  if (!snap.has_next) {
    await opened.context.close().catch(() => {});
    snap.session_id = null;
    return snap;
  }
  const id = crypto.randomUUID();
  sessions.set(id, { context: opened.context, page: opened.page, lastUsed: Date.now() });
  snap.session_id = id;
  return snap;
}

function readJson(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      try {
        const raw = Buffer.concat(chunks).toString("utf8") || "{}";
        resolve(JSON.parse(raw));
      } catch (err) {
        reject(err);
      }
    });
    req.on("error", reject);
  });
}

const server = http.createServer(async (req, res) => {
  const send = (code, obj) => {
    const body = JSON.stringify(obj);
    res.writeHead(code, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) });
    res.end(body);
  };

  try {
    if (req.method === "GET" && req.url === "/health") {
      await getBrowser();
      return send(200, { ok: true, browser: BROWSER_NAME, headless: HEADLESS });
    }
    if (req.method === "POST" && req.url === "/scrape-list") {
      const payload = await readJson(req);
      const result = await scrapeList(payload);
      return send(200, result);
    }
    if (req.method === "POST" && req.url === "/fetch-page") {
      const payload = await readJson(req);
      const result = await fetchPage(payload);
      return send(200, result);
    }
    send(404, { error: "not found" });
  } catch (err) {
    const status = err.status || 500;
    send(status, { error: err.message || String(err), cards: [], pages_visited: 0 });
  }
});

server.listen(PORT, "0.0.0.0", () => {
  console.log(`playwright worker on :${PORT} browser=${BROWSER_NAME} headless=${HEADLESS}`);
});
