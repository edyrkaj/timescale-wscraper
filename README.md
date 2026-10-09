# Timescale Job Scraper

Generic Docker-local job-board scraper: paste a listing URL + date range, scrape jobs into TimescaleDB, control a sequential queue from a web UI.

The **UI** and **API** run as separate services.

## Quick start

```bash
cp .env.example .env   # optional; defaults work out of the box
docker compose up --build
```

Open [http://localhost:3000](http://localhost:3000) (UI). API is on [http://localhost:8080](http://localhost:8080).

| Service | URL / port |
|---------|------------|
| **UI** | `localhost:3000` |
| **API** | `localhost:8080` |
| **TimescaleDB** | `localhost:5432` — user `postgres` / password `password` / db `scrapers_db` |
| **Playwright worker** | `localhost:3001` (DOM listing scrapes) |

Env: see `.env.example` (copy to `.env`; `.env` is gitignored).

## Local (split processes)

```bash
# DB + Playwright
docker compose up timescaledb playwright -d

# API
export DATABASE_URL="postgres://postgres:password@localhost:5432/scrapers_db"
export PLAYWRIGHT_URL="http://localhost:3001"
cargo run

# UI (separate terminal) — serves static/ on :3000; auto-targets API on :8080
docker compose up ui -d
# or: cd static && python3 -m http.server 3000
```

Override the API URL in [`static/config.js`](static/config.js) if needed:

```js
window.API_BASE = "http://127.0.0.1:8080";
```

## Scrape patterns (list → detail)

Sites like [Tirana E-rekrutim](https://rekrutimi.tirana.al/shpalljet) list jobs on one page and put full data on `/shpalljet/{uuid}`.

Patterns are stored in Timescale (`scrape_patterns`) and editable in the UI:

1. Open **http://localhost:3000**
2. Expand **Scrape patterns**, create/edit a pattern (`url_match` + JSON config)
3. Start a scrape with a matching listing URL — the pattern is auto-attached

Enable **AI Scraper** and pick a provider from the dropdown (**Anthropic**, **Gemini**, or **Local vLLM (Qwen)**).

Local Qwen runs in vLLM outside Compose. On a 32 GB Mac, use Qwen3-8B. Do not pass `--api-key`. The app calls `POST /v1/chat/completions` with no OpenAI key (`VLLM_BASE_URL`, `VLLM_MODEL` in `.env.example`).

```bash
vllm serve Qwen/Qwen3-8B \
  --max-model-len 4096 \
  --gpu-memory-utilization 0.7
```

The Tirana pattern is seeded automatically. It uses the public list API, then opens each detail page for title/company/description enrichment.

## API

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Health check |
| POST | `/api/jobs` | Enqueue scrape |
| GET | `/api/jobs` | List jobs |
| POST | `/api/jobs/{id}/stop` | Stop job |
| POST | `/api/jobs/{id}/restart` | Re-enqueue |
| GET | `/api/jobs/stream` | SSE progress |
| GET/POST | `/api/patterns` | List / create scrape patterns |
| PUT/DELETE | `/api/patterns/{id}` | Update / delete pattern |
| GET | `/api/items` | Recent scraped items |
