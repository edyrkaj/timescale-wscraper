const form = document.getElementById("scrape-form");
const stopBtn = document.getElementById("stop-btn");
const restartBtn = document.getElementById("restart-btn");
const statusLabel = document.getElementById("status-label");
const counters = document.getElementById("counters");
const message = document.getElementById("message");
const progressBar = document.getElementById("progress-bar");
const jobsEl = document.getElementById("jobs");
const itemsEl = document.getElementById("items");

let selectedJobId = null;
let currentRunningId = null;

function todayIso() {
  return new Date().toISOString().slice(0, 10);
}

function daysAgoIso(n) {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return d.toISOString().slice(0, 10);
}

document.getElementById("to_date").value = todayIso();
document.getElementById("from_date").value = daysAgoIso(30);

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const body = {
    url: document.getElementById("url").value.trim(),
    from_date: document.getElementById("from_date").value,
    to_date: document.getElementById("to_date").value,
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
  message.textContent = `Enqueued ${data.id}`;
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

  // Soft progress toward max items (2000)
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
    li.innerHTML = `<div><strong>${job.status}</strong> · ${job.scraped_count} items · ${job.pages_visited} pages</div>
      <div class="meta">${job.listing_url}</div>
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
refreshJobs();
refreshItems();
setInterval(() => {
  refreshJobs();
  refreshItems();
}, 5000);
