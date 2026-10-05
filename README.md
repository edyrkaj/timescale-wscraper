# Timescale Job Scraper

Generic Docker-local job-board scraper: paste a listing URL + date range, scrape with headless Chromium, store jobs in TimescaleDB, control a sequential queue from a web UI.

## Quick start

```bash
docker compose up --build
```

Open [http://localhost:8080](http://localhost:8080).

- **TimescaleDB:** `localhost:5432` — user `postgres` / password `password` / db `scrapers_db`
- **App:** `localhost:8080`

## Local (without Docker app)

```bash
# Start only the DB
docker compose up timescaledb -d

export DATABASE_URL="postgres://postgres:password@localhost:5432/scrapers_db"
export CHROME_PATH="$(which chromium || which google-chrome || which chromium-browser)"
cargo run
```

## Schema

See `schema.sql` for `scraped_items` (hypertable) and `scrape_jobs` (queue).

## API

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/` | UI |
| POST | `/api/jobs` | Enqueue scrape |
| GET | `/api/jobs` | List jobs |
| POST | `/api/jobs/{id}/stop` | Stop job |
| POST | `/api/jobs/{id}/restart` | Re-enqueue |
| GET | `/api/jobs/stream` | SSE progress |
| GET | `/api/items` | Recent scraped items |
