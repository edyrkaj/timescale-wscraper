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
    send(404, { error: "not found" });
  } catch (err) {
    const status = err.status || 500;
    send(status, { error: err.message || String(err), cards: [], pages_visited: 0 });
  }
});

server.listen(PORT, "0.0.0.0", () => {
  console.log(`playwright worker on :${PORT} browser=${BROWSER_NAME} headless=${HEADLESS}`);
});
