const form = document.getElementById("scrape-form");
const stopBtn = document.getElementById("stop-btn");
const restartBtn = document.getElementById("restart-btn");
const statusLabel = document.getElementById("status-label");
const counters = document.getElementById("counters");
const message = document.getElementById("message");
const progressBar = document.getElementById("progress-bar");
const jobsEl = document.getElementById("jobs");
const itemsEl = document.getElementById("items");
const patternsEl = document.getElementById("patterns");
const patternSelect = document.getElementById("pattern_id");
const patternForm = document.getElementById("pattern-form");
const patternConfig = document.getElementById("pattern_config");
const useAi = document.getElementById("use_ai");

function syncAiToggle() {
  const on = useAi.checked;
  patternSelect.disabled = on;
  if (on) patternSelect.value = "";
}

useAi.addEventListener("change", syncAiToggle);

let selectedJobId = null;
let currentRunningId = null;
let editingPatternId = null;
let patternsCache = [];

const DEFAULT_PATTERN = {
  list: {
    source: "api",
    api: {
      url_template: "https://rekrutimi.tirana.al/api/api/Job/public-announcements?PageNumber={page}&PageSize={page_size}",
      page_size: 50,
      items_path: "$",
      id_path: "id",
      published_at_path: "job.0.publishedDate",
      title_path: "job.0.jobPositionsResponse.0.positionName",
      company_literal: "Bashkia Tiranë",
      location_path: "job.0.jobPositionsResponse.0.organisationalUnit",
      salary_path: "job.0.jobPositionsResponse.0.categoryName",
      detail_url_template: "https://rekrutimi.tirana.al/shpalljet/{id}"
    }
  },
  detail: {
    source: "page",
    wait_ms: 2500,
    fields: {
      title_label: "Pozicioni",
      company_label: "Institucioni",
      description_selector: "div.card.p-3",
      date_regex: "\\b(\\d{1,2}/\\d{1,2}/20\\d{2})\\b"
    }
  }
};

patternConfig.value = JSON.stringify(DEFAULT_PATTERN, null, 2);

function todayIso() {
  return toIsoDate(new Date());
}

function daysAgoIso(n) {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return toIsoDate(d);
}

const MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

let openPickerClose = null;

function parseIsoDate(s) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(s || "")) return null;
  const [y, m, d] = s.split("-").map(Number);
  const dt = new Date(y, m - 1, d);
  if (dt.getFullYear() !== y || dt.getMonth() !== m - 1 || dt.getDate() !== d) return null;
  return dt;
}

function toIsoDate(dt) {
  const y = dt.getFullYear();
  const m = String(dt.getMonth() + 1).padStart(2, "0");
  const d = String(dt.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function yearRange() {
  const y = new Date().getFullYear();
  return { min: y - 10, max: y + 1 };
}

function initDatePicker(input) {
  const wrap = input.closest(".date-field");
  if (!wrap || wrap.querySelector(".date-picker")) return;

  let lastValid = input.value;
  const popup = document.createElement("div");
  popup.className = "date-picker";
  popup.innerHTML = `
    <div class="date-picker-header">
      <button type="button" data-nav="-1" aria-label="Previous month">‹</button>
      <select data-month></select>
      <select data-year></select>
      <button type="button" data-nav="1" aria-label="Next month">›</button>
    </div>
    <div class="date-picker-weekdays">${WEEKDAYS.map((w) => `<span>${w}</span>`).join("")}</div>
    <div class="date-picker-grid"></div>`;
  wrap.appendChild(popup);

  const monthSel = popup.querySelector("[data-month]");
  const yearSel = popup.querySelector("[data-year]");
  const grid = popup.querySelector(".date-picker-grid");
  const { min, max } = yearRange();
  MONTHS.forEach((name, i) => {
    const opt = document.createElement("option");
    opt.value = String(i);
    opt.textContent = name;
    monthSel.appendChild(opt);
  });
  for (let y = min; y <= max; y++) {
    const opt = document.createElement("option");
    opt.value = String(y);
    opt.textContent = String(y);
    yearSel.appendChild(opt);
  }

  let view = parseIsoDate(input.value) || new Date();

  function close() {
    popup.classList.remove("is-open");
    if (openPickerClose === close) openPickerClose = null;
  }

  function open() {
    if (openPickerClose && openPickerClose !== close) openPickerClose();
    view = parseIsoDate(input.value) || new Date();
    lastValid = parseIsoDate(input.value) ? input.value : lastValid;
    render();
    popup.classList.add("is-open");
    openPickerClose = close;
  }

  function clampView() {
    let y = view.getFullYear();
    let m = view.getMonth();
    if (y < min) { y = min; m = 0; }
    if (y > max) { y = max; m = 11; }
    view = new Date(y, m, 1);
  }

  function render() {
    clampView();
    monthSel.value = String(view.getMonth());
    yearSel.value = String(view.getFullYear());
    const selected = parseIsoDate(input.value);
    const year = view.getFullYear();
    const month = view.getMonth();
    const first = new Date(year, month, 1);
    const startPad = (first.getDay() + 6) % 7;
    const daysInMonth = new Date(year, month + 1, 0).getDate();
    const cells = [];
    for (let i = 0; i < startPad; i++) {
      cells.push(`<button type="button" class="date-picker-day is-outside" disabled tabindex="-1"></button>`);
    }
    for (let d = 1; d <= daysInMonth; d++) {
      const iso = toIsoDate(new Date(year, month, d));
      const sel = selected && toIsoDate(selected) === iso ? " is-selected" : "";
      cells.push(`<button type="button" class="date-picker-day${sel}" data-day="${d}">${d}</button>`);
    }
    grid.innerHTML = cells.join("");
  }

  monthSel.addEventListener("change", () => {
    view = new Date(Number(yearSel.value), Number(monthSel.value), 1);
    render();
  });
  yearSel.addEventListener("change", () => {
    view = new Date(Number(yearSel.value), Number(monthSel.value), 1);
    render();
  });
  popup.querySelectorAll("[data-nav]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const delta = Number(btn.dataset.nav);
      view = new Date(view.getFullYear(), view.getMonth() + delta, 1);
      render();
    });
  });
  grid.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-day]");
    if (!btn) return;
    const d = Number(btn.dataset.day);
    const next = new Date(view.getFullYear(), view.getMonth(), d);
    input.value = toIsoDate(next);
    lastValid = input.value;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    close();
  });

  // Avoid input blur-restore racing day clicks; allow selects to focus normally
  popup.addEventListener("mousedown", (e) => {
    if (e.target.closest("select")) return;
    e.preventDefault();
  });

  const toggle = wrap.querySelector(".date-picker-toggle");
  if (toggle) {
    toggle.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (popup.classList.contains("is-open")) close();
      else open();
    });
  }

  // Typing is free; only validate when leaving the field
  input.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      close();
      return;
    }
    // Keep typing uninterrupted — close calendar if open
    if (popup.classList.contains("is-open") && e.key.length === 1) close();
  });
  input.addEventListener("blur", () => {
    const parsed = parseIsoDate(input.value.trim());
    if (parsed) {
      input.value = toIsoDate(parsed);
      lastValid = input.value;
    } else if (input.value.trim() === "") {
      // leave empty so HTML required can catch it on submit
      lastValid = "";
    } else {
      input.value = lastValid;
    }
  });

  document.addEventListener("click", (e) => {
    if (!popup.classList.contains("is-open")) return;
    if (wrap.contains(e.target)) return;
    close();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && popup.classList.contains("is-open")) close();
  });
}

document.getElementById("to_date").value = todayIso();
document.getElementById("from_date").value = daysAgoIso(30);
initDatePicker(document.getElementById("from_date"));
initDatePicker(document.getElementById("to_date"));

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const patternId = useAi.checked ? null : (patternSelect.value || null);
  const body = {
    url: document.getElementById("url").value.trim(),
    from_date: document.getElementById("from_date").value,
    to_date: document.getElementById("to_date").value,
    pattern_id: patternId,
    use_ai: useAi.checked,
  };
  const res = await fetch("/api/jobs", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json();
  if (!res.ok) {
    message.textContent = data.error || "Failed to enqueue";
    return;
  }
  selectedJobId = data.id;
  message.textContent = `Enqueued ${data.id}${
    data.use_ai ? " (AI scrape)" : data.pattern_id ? " (pattern attached)" : ""
  }`;
  restartBtn.disabled = false;
  await refreshJobs();
});

stopBtn.addEventListener("click", async () => {
  const id = currentRunningId || selectedJobId;
  if (!id) return;
  const res = await fetch(`/api/jobs/${id}/stop`, { method: "POST" });
  const data = await res.json();
  message.textContent = data.error || "Stop requested";
  await refreshJobs();
});

restartBtn.addEventListener("click", async () => {
  if (!selectedJobId) return;
  const res = await fetch(`/api/jobs/${selectedJobId}/restart`, { method: "POST" });
  const data = await res.json();
  if (!res.ok) {
    message.textContent = data.error || "Restart failed";
    return;
  }
  selectedJobId = data.id;
  message.textContent = `Restarted as ${data.id}`;
  await refreshJobs();
});

patternForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  let config;
  try {
    config = JSON.parse(patternConfig.value);
  } catch (err) {
    message.textContent = `Invalid JSON: ${err.message}`;
    return;
  }
  const body = {
    name: document.getElementById("pattern_name").value.trim(),
    url_match: document.getElementById("pattern_match").value.trim(),
    config,
    enabled: true,
  };
  const url = editingPatternId ? `/api/patterns/${editingPatternId}` : "/api/patterns";
  const method = editingPatternId ? "PUT" : "POST";
  const res = await fetch(url, {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json();
  if (!res.ok) {
    message.textContent = data.error || "Failed to save pattern";
    return;
  }
  message.textContent = editingPatternId ? "Pattern updated" : "Pattern created";
  editingPatternId = null;
  resetPatternForm();
  await refreshPatterns();
});

document.getElementById("pattern-reset").addEventListener("click", () => {
  editingPatternId = null;
  resetPatternForm();
});

function resetPatternForm() {
  document.getElementById("pattern_name").value = "";
  document.getElementById("pattern_match").value = "";
  patternConfig.value = JSON.stringify(DEFAULT_PATTERN, null, 2);
}

function applyProgress(evt) {
  const scraped = evt.scraped_count || (evt.current_job && evt.current_job.scraped_count) || 0;
  const queueDepth = evt.queue_depth || 0;
  const status = (evt.current_job && evt.current_job.status) || evt.status || "idle";
  currentRunningId = evt.current_job && evt.current_job.status === "running"
    ? evt.current_job.id
    : null;

  statusLabel.textContent = status.charAt(0).toUpperCase() + status.slice(1);
  counters.textContent = `Queue: ${queueDepth} · Scraped: ${scraped}`;
  message.textContent = evt.message || "";

  const running = status === "running";
  stopBtn.disabled = !running && !currentRunningId;
  progressBar.classList.toggle("active", running);

  const pct = Math.min(92, Math.max(8, (scraped / 2000) * 100 + (running ? 8 : 0)));
  progressBar.style.width = `${pct}%`;
  if (!running && status === "completed") progressBar.style.width = "100%";
  if (!running && (status === "idle" || status === "stopped" || status === "failed")) {
    if (scraped === 0) progressBar.style.width = "8%";
  }
}

async function refreshJobs() {
  const res = await fetch("/api/jobs");
  const jobs = await res.json();
  jobsEl.innerHTML = "";
  for (const job of jobs) {
    const li = document.createElement("li");
    if (job.id === selectedJobId) li.classList.add("selected");
    const patternName = job.use_ai
      ? "AI scraper"
      : patternsCache.find((p) => p.id === job.pattern_id)?.name || (job.pattern_id ? job.pattern_id.slice(0, 8) : "heuristic");
    li.innerHTML = `<div><strong>${job.status}</strong> · ${job.scraped_count} items · ${job.pages_visited} pages · ${escapeHtml(patternName)}</div>
      <div class="meta">${escapeHtml(job.listing_url)}</div>
      <div class="meta">${job.from_date} → ${job.to_date} · ${job.id}</div>`;
    li.addEventListener("click", () => {
      selectedJobId = job.id;
      restartBtn.disabled = false;
      refreshJobs();
    });
    jobsEl.appendChild(li);
  }
}

async function refreshItems() {
  const res = await fetch("/api/items");
  const items = await res.json();
  itemsEl.innerHTML = "";
  for (const item of items) {
    const li = document.createElement("li");
    li.innerHTML = `<div><a href="${item.url}" target="_blank" rel="noopener">${escapeHtml(item.title)}</a></div>
      <div class="meta">${escapeHtml(item.company || "—")} · ${escapeHtml(item.location || "—")} · ${item.item_timestamp}</div>`;
    itemsEl.appendChild(li);
  }
}

async function refreshPatterns() {
  const res = await fetch("/api/patterns");
  patternsCache = await res.json();
  const previous = patternSelect.value;
  patternSelect.innerHTML = `<option value="">Auto / heuristic</option>`;
  patternsEl.innerHTML = "";
  for (const p of patternsCache) {
    const opt = document.createElement("option");
    opt.value = p.id;
    opt.textContent = `${p.name} (${p.url_match})`;
    patternSelect.appendChild(opt);

    const li = document.createElement("li");
    li.innerHTML = `<div><strong>${escapeHtml(p.name)}</strong> ${p.enabled ? "" : "(disabled)"}</div>
      <div class="meta">match: ${escapeHtml(p.url_match)}</div>
      <div class="pattern-actions">
        <button type="button" data-edit="${p.id}">Edit</button>
        <button type="button" class="secondary" data-del="${p.id}">Delete</button>
      </div>`;
    patternsEl.appendChild(li);
  }
  if (previous) patternSelect.value = previous;

  patternsEl.querySelectorAll("[data-edit]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const p = patternsCache.find((x) => x.id === btn.dataset.edit);
      if (!p) return;
      editingPatternId = p.id;
      document.getElementById("pattern_name").value = p.name;
      document.getElementById("pattern_match").value = p.url_match;
      patternConfig.value = JSON.stringify(p.config, null, 2);
      message.textContent = `Editing pattern ${p.name}`;
    });
  });
  patternsEl.querySelectorAll("[data-del]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!confirm("Delete this pattern?")) return;
      await fetch(`/api/patterns/${btn.dataset.del}`, { method: "DELETE" });
      await refreshPatterns();
    });
  });
}

function escapeHtml(s) {
  return String(s)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function connectSse() {
  const es = new EventSource("/api/jobs/stream");
  es.addEventListener("progress", (e) => {
    try {
      applyProgress(JSON.parse(e.data));
      refreshJobs();
      refreshItems();
    } catch (_) {}
  });
  es.onerror = () => {
    es.close();
    setTimeout(connectSse, 2000);
  };
}

connectSse();
refreshPatterns().then(() => {
  refreshJobs();
  refreshItems();
});
setInterval(() => {
  refreshJobs();
  refreshItems();
}, 5000);
