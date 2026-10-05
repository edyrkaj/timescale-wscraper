# Timescale Job Scraper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Docker-local Axum + Chromium job scraper with TimescaleDB, queue UI, SSE progress, stop/restart, incremental upserts.

**Architecture:** Single Rust app (Axum API + static UI + in-process sequential job queue + chromiumoxide scraper) and TimescaleDB via Docker Compose.

**Tech Stack:** Rust 2021, Axum, tokio, sqlx, chromiumoxide, chrono, serde, tower-http (static files), Docker Compose, TimescaleDB.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-10-05-timescale-job-scraper-design.md`
- Soft caps: max 50 pages, max 2000 items per job
- Job statuses: `queued` | `running` | `stopped` | `completed` | `failed`
- Bind default: `0.0.0.0:8080`
- First-class job columns only (no core JSONB)
- Sequential queue (one active scrape at a time)

---

## File Structure

```
schema.sql                          # scraped_items + scrape_jobs + hypertable
docker-compose.yml                  # timescaledb + app
Dockerfile                          # Rust build + Chromium runtime deps
Cargo.toml                          # deps
src/main.rs                         # boot, pool, routes, spawn worker
src/models.rs                       # ScrapedItem, ScrapeJob, DTOs
src/db.rs                           # upserts, job CRUD, watermark
src/queue.rs                        # sequential worker + cancel tokens
src/scraper.rs                      # chromiumoxide listing scrape + heuristics
src/api.rs                          # REST + SSE handlers
static/index.html                   # UI
static/app.js                       # form, SSE, progress
static/style.css                    # layout + animated progress
README.md                           # docker compose usage
```

---

### Task 1: Schema + Docker foundation

**Files:**
- Modify: `schema.sql`
- Create: `docker-compose.yml`, `Dockerfile`, `.dockerignore`
- Modify: `README.md`

- [ ] **Step 1: Replace `schema.sql` with full schema**

```sql
CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TABLE IF NOT EXISTS scraped_items (
    source_id TEXT NOT NULL,
    external_id TEXT NOT NULL,
    item_timestamp TIMESTAMPTZ NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    company TEXT,
    location TEXT,
    salary TEXT,
    description TEXT,
    PRIMARY KEY (source_id, external_id, item_timestamp)
);

SELECT create_hypertable('scraped_items', 'item_timestamp', if_not_exists => TRUE);

CREATE TABLE IF NOT EXISTS scrape_jobs (
    id UUID PRIMARY KEY,
    listing_url TEXT NOT NULL,
    from_date DATE NOT NULL,
    to_date DATE NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    scraped_count INT NOT NULL DEFAULT 0,
    pages_visited INT NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS scrape_jobs_status_created_idx
  ON scrape_jobs (status, created_at);
```

- [ ] **Step 2: Add `docker-compose.yml` + `Dockerfile`**

Compose: `timescaledb/timescaledb:latest-pg16` on 5432, init mount `./schema.sql` → `/docker-entrypoint-initdb.d/01-schema.sql`, volume for data. App service builds Dockerfile, depends on healthy DB, `DATABASE_URL=postgres://postgres:password@timescaledb:5432/scrapers_db`, ports `8080:8080`, `shm_size: '1gb'` for Chromium.

Dockerfile multi-stage: `rust:1.83` builder → `debian:bookworm-slim` with chromium + deps, copy binary + `static/`.

- [ ] **Step 3: Update README with `docker compose up --build`**

- [ ] **Step 4: Commit**

```bash
git add schema.sql docker-compose.yml Dockerfile .dockerignore README.md
git commit -m "chore: add Timescale schema and Docker Compose stack"
```

---

### Task 2: Models + DB layer

**Files:**
- Create: `src/models.rs`, `src/db.rs`
- Modify: `Cargo.toml`, `src/main.rs` (module wiring only)

**Interfaces:**
- Produces: `ScrapedItem`, `ScrapeJob`, `CreateJobRequest`, `JobStatus`
- Produces: `Db::save_items`, `Db::create_job`, `Db::list_jobs`, `Db::claim_next_job`, `Db::update_job_progress`, `Db::finish_job`, `Db::get_job`, `Db::stop_job`

- [ ] **Step 1: Update Cargo.toml deps**

Add: `axum`, `tower-http` (fs, cors), `uuid` (v4, serde), `chromiumoxide`, `futures-util`, `tokio-stream`, `sha2`, `url`, `regex`, `tracing`, `tracing-subscriber`. Keep existing sqlx/tokio/chrono/serde/anyhow.

- [ ] **Step 2: Implement `src/models.rs` and `src/db.rs`**

`external_id` = hex sha256 of canonical job URL. Upsert SQL matches spec columns. `claim_next_job` uses `UPDATE ... SET status='running' WHERE id = (SELECT id FROM scrape_jobs WHERE status='queued' ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING *`.

- [ ] **Step 3: Smoke-compile**

Run: `cargo check`
Expected: compiles (main may still be stub)

- [ ] **Step 4: Commit**

```bash
git commit -m "feat: add job/item models and Timescale DB helpers"
```

---

### Task 3: Queue worker + scraper

**Files:**
- Create: `src/queue.rs`, `src/scraper.rs`

**Interfaces:**
- Consumes: `Db` methods from Task 2
- Produces: `QueueHandle { enqueue_signal, cancel_current, spawn }`
- Produces: `scrape_listing(browser, url, from, to, cancel, on_batch) -> Result<ScrapeStats>`

- [ ] **Step 1: Implement sequential `queue.rs`**

Loop: wait for signal or poll every 2s → claim job → run scraper with `CancellationToken` → finish status. On cancel: `stopped`. Update `scraped_count` / `pages_visited` after each batch.

- [ ] **Step 2: Implement `scraper.rs` with chromiumoxide**

Navigate, wait, extract anchors matching job URL regexes, parse nearby text for title/company/location/salary/date, filter by date window, call `on_batch`, paginate until caps/cancel/date stop.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat: add sequential scrape queue and Chromium heuristics"
```

---

### Task 4: API + SSE + static UI

**Files:**
- Create: `src/api.rs`, `static/index.html`, `static/app.js`, `static/style.css`
- Modify: `src/main.rs`

**Interfaces:**
- Routes per spec API table
- SSE event JSON: `{ current_job, queue_depth, scraped_count, status }`

- [ ] **Step 1: Implement Axum routes + SSE broadcaster**

Shared `AppState { db, queue, events: broadcast::Sender<ProgressEvent> }`.

- [ ] **Step 2: Build UI with form, progress bar, counters, stop/restart**

- [ ] **Step 3: Wire `main.rs` — connect pool, ensure schema optional, spawn queue, serve static + API**

- [ ] **Step 4: Commit**

```bash
git commit -m "feat: add Axum API, SSE progress, and scrape UI"
```

---

### Task 5: End-to-end Docker verification

- [ ] **Step 1: `docker compose up --build -d`**
- [ ] **Step 2: Open `http://localhost:8080`, enqueue a sample public listing URL with date range**
- [ ] **Step 3: Confirm progress SSE updates and rows in `scraped_items`**
- [ ] **Step 4: Test stop + restart**
- [ ] **Step 5: Commit any fixes + final README polish**

---

## Spec coverage check

| Spec requirement | Task |
|---|---|
| Expanded scraped_items columns + hypertable | 1 |
| scrape_jobs queue table | 1 |
| Docker Compose Timescale + app + Chromium | 1, 5 |
| Sequential queue, stop, restart | 3, 4 |
| Browser scrape + date filter + soft caps | 3 |
| Incremental upserts | 2, 3 |
| UI + progress bar + counters + SSE | 4 |
| REST API | 4 |
