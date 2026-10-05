# Timescale Job Scraper — Design Spec

**Date:** 2026-10-05  
**Status:** Approved

## Goal

Build a Docker-local, generic job-board scraper: user enters a listing URL and date range; the system scrapes with headless Chromium, filters by date, upserts job posts into TimescaleDB incrementally, and exposes a simple web UI with queue, progress, stop, and restart.

## Decisions

| Topic | Choice |
|---|---|
| Scraping | Headless Chromium (browser-based) |
| App shape | Single Rust (Axum) service: API + static UI |
| Job concurrency | Sequential queue (enqueue many, run one at a time) |
| Job fields | First-class columns (no JSONB blob for core fields) |
| Date filtering | Filter after extract; soft max pages/items; stop when dates fall before `from` |
| Architecture | Monolith Rust + Chromium in app image + TimescaleDB Compose service |
| Progress | Server-Sent Events (SSE) |

## Architecture

Docker Compose services:

1. **`timescaledb`** — TimescaleDB; schema applied on first boot via mounted SQL.
2. **`app`** — Rust Axum binary, static UI, Chromium (`chromiumoxide`).

Flow:

1. UI submits URL + `[from, to]` → `POST /api/jobs` enqueues a row in `scrape_jobs`.
2. In-process queue worker claims the next `queued` job and marks it `running`.
3. Worker opens Chromium, loads listing page(s), extracts job cards, filters by date window, upserts each batch into `scraped_items`.
4. UI receives live progress via `GET /api/jobs/stream` (status, scraped count, queue depth).
5. **Stop** cancels the current job via cancellation token → status `stopped` (rows already saved remain).
6. **Restart** re-enqueues the same URL/dates as a new `queued` job (idempotent via upserts).

```
Browser UI ──REST/SSE──► Axum App ──queue──► Scraper Worker ──► Chromium
                              │                    │
                              └──── sqlx ──────────┴──► TimescaleDB
```

## Data model

### `scraped_items` (hypertable on `item_timestamp`)

| Column | Type | Notes |
|---|---|---|
| `source_id` | TEXT | Host of listing URL |
| `external_id` | TEXT | Stable hash of job URL (or site id if present) |
| `item_timestamp` | TIMESTAMPTZ | `posted_at`, or scrape time if date missing |
| `title` | TEXT | Required |
| `url` | TEXT | Job detail URL |
| `company` | TEXT | Nullable |
| `location` | TEXT | Nullable |
| `salary` | TEXT | Nullable (free text) |
| `description` | TEXT | Nullable (snippet or full) |

Primary key: `(source_id, external_id, item_timestamp)`.  
Upsert on conflict updates title/url/company/location/salary/description.

### `scrape_jobs`

| Column | Type | Notes |
|---|---|---|
| `id` | UUID | PK |
| `listing_url` | TEXT | User-submitted URL |
| `from_date` | DATE | Inclusive |
| `to_date` | DATE | Inclusive |
| `status` | TEXT | `queued` \| `running` \| `stopped` \| `completed` \| `failed` |
| `scraped_count` | INT | Items saved this job |
| `pages_visited` | INT | Pagination counter |
| `last_error` | TEXT | Nullable |
| `created_at` | TIMESTAMPTZ | |
| `started_at` | TIMESTAMPTZ | Nullable |
| `finished_at` | TIMESTAMPTZ | Nullable |

## API

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/` | Static UI |
| `POST` | `/api/jobs` | Enqueue `{ url, from_date, to_date }` |
| `GET` | `/api/jobs` | List recent jobs |
| `POST` | `/api/jobs/{id}/stop` | Cancel if running / mark stopped if queued |
| `POST` | `/api/jobs/{id}/restart` | Enqueue clone of URL/dates |
| `GET` | `/api/jobs/stream` | SSE: current job, counters, queue depth |
| `GET` | `/api/items` | Optional recent scraped items (debug/UI table) |

## UI

Single page served by Axum:

- Inputs: website/page URL, start date, end date
- Actions: Start (enqueue), Stop (cancel current), Restart (re-queue selected/last)
- Animated progress bar + counters: queue depth, current status, items scraped this run
- Live updates via SSE
- Functional, clear layout (not a marketing landing page)

## Scrape heuristics

1. Navigate to listing URL; wait for network idle and/or common job-list selectors.
2. Extract candidate cards: links/text matching job URL patterns (`/job`, `/jobs/`, `/position`, etc.) and nearby title/company/location/date/salary from DOM.
3. Parse `posted_at` when possible; keep only items in `[from, to]`; upsert each batch immediately.
4. Paginate (Next link, `?page=`, or load-more click) until dates fall before `from`, max pages hit, max items hit, or cancel.
5. Soft caps (defaults): max 50 pages, max 2000 items per job.
6. Consecutive page failures: after N failures mark job `failed` with `last_error`.

## Error handling & cancellation

- Cancel → status `stopped`; keep already-upserted rows.
- Failed job → `failed` + `last_error`; does not block subsequent queued jobs.
- Missing parseable date → use scrape time as `item_timestamp` but still apply soft caps; prefer excluding from strict date filter only when date is known (if date unknown, include within soft caps and tag via description/company nulls as needed).

**Date rule (explicit):** If a job card has a parseable date outside `[from, to]`, drop it. If no date is parseable, include it only while still under soft caps (best-effort for unknown boards).

## Docker

- `docker-compose.yml`: `timescaledb` + `app`
- App image: Rust build + Chromium dependencies
- Env: `DATABASE_URL`, `BIND_ADDR` (default `0.0.0.0:8080`)
- Schema init mount for Timescale

## Out of scope (v1)

- Per-site CSS selector configuration UI
- Multi-worker parallel Chromium
- Auth / multi-user
- Site-native date query params
- Production hardening (rate limits, proxies, CAPTCHA)

## Success criteria

- `docker compose up` brings up Timescale + app with Chromium
- User can enqueue a URL + date range and see progress + scraped count
- Stop and restart work; data is saved incrementally and survives restart without duplicates
- Job posts queryable in Timescale with first-class columns
