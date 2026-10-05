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
    finished_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS scrape_jobs_status_created_idx
    ON scrape_jobs (status, created_at);
