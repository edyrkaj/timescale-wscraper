-- TimescaleDB Schema Setup
-- Hypertables require that unique indexes include the partitioning column (item_timestamp).

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
    finished_at TIMESTAMPTZ,
    pattern_id UUID,
    use_ai BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS scrape_jobs_status_created_idx
    ON scrape_jobs (status, created_at);

CREATE TABLE IF NOT EXISTS scrape_patterns (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    url_match TEXT NOT NULL,
    config JSONB NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS scrape_patterns_url_match_idx
    ON scrape_patterns (url_match);

-- Seed: Tirana E-rekrutim (list API → detail pages)
INSERT INTO scrape_patterns (id, name, url_match, config, enabled)
VALUES (
    'a1111111-1111-4111-8111-111111111111',
    'Tirana E-rekrutim',
    'rekrutimi.tirana.al/shpalljet',
    '{
      "list": {
        "source": "api",
        "api": {
          "url_template": "https://rekrutimi.tirana.al/api/api/Job/public-announcements?PageNumber={page}&PageSize={page_size}",
          "page_size": 50,
          "items_path": "$",
          "id_path": "id",
          "published_at_path": "job.0.publishedDate",
          "title_path": "job.0.jobPositionsResponse.0.positionName",
          "company_literal": "Bashkia Tiranë",
          "location_path": "job.0.jobPositionsResponse.0.organisationalUnit",
          "salary_path": "job.0.jobPositionsResponse.0.categoryName",
          "detail_url_template": "https://rekrutimi.tirana.al/shpalljet/{id}"
        }
      },
            "detail": {
                "source": "api",
                "api_url_template": "https://rekrutimi.tirana.al/api/api/Job/get-positions-by-job/{id}",
                "description_path": "0.positionDescription"
            }
    }'::jsonb,
    TRUE
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO scrape_patterns (id, name, url_match, config, enabled)
VALUES (
    'a2222222-2222-4222-8222-222222222222',
    'DuaPune listing',
    'duapune.com',
    '{
      "list": {
        "source": "dom",
        "dom": {
          "wait_for": "div.job-listing",
          "wait_ms": 5000,
          "card_selector": "div.job-listing",
          "title_selector": "h1.job-title > a",
          "company_selector": "h1.job-title small a",
          "location_selector": "span.location",
          "date_selector": "span.time",
          "item_link_regex": "/jobs/\\d+"
        }
      },
      "detail": { "source": "none" }
    }'::jsonb,
    TRUE
)
ON CONFLICT (id) DO NOTHING;
