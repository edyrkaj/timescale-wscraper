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

## Scrape patterns (list → detail)

Sites like [Tirana E-rekrutim](https://rekrutimi.tirana.al/shpalljet) list jobs on one page and put full data on `/shpalljet/{uuid}`.

Patterns are stored in Timescale (`scrape_patterns`) and editable in the UI:

1. Open **http://localhost:8080**
2. Under **Scrape patterns**, create/edit a pattern (`url_match` + JSON config)
3. Start a scrape with a matching listing URL — the pattern is auto-attached

The Tirana pattern is seeded automatically. It uses the public list API, then opens each detail page for title/company/description enrichment.

## API

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/` | UI |
| POST | `/api/jobs` | Enqueue scrape |
| GET | `/api/jobs` | List jobs |
| POST | `/api/jobs/{id}/stop` | Stop job |
| POST | `/api/jobs/{id}/restart` | Re-enqueue |
| GET | `/api/jobs/stream` | SSE progress |
| GET/POST | `/api/patterns` | List / create scrape patterns |
| PUT/DELETE | `/api/patterns/{id}` | Update / delete pattern |
| GET | `/api/items` | Recent scraped items |
