use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{JobStatus, ScrapeJob, ScrapePattern, ScrapedItem, UpsertPatternRequest};

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Idempotent migrations for existing Docker volumes that already ran an older schema.sql.
    pub async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scrape_patterns (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL,
                url_match TEXT NOT NULL,
                config JSONB NOT NULL,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            ALTER TABLE scrape_jobs
            ADD COLUMN IF NOT EXISTS pattern_id UUID
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            ALTER TABLE scrape_jobs
            ADD COLUMN IF NOT EXISTS use_ai BOOLEAN NOT NULL DEFAULT FALSE
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            ALTER TABLE scrape_jobs
            ADD COLUMN IF NOT EXISTS ai_provider TEXT
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            ALTER TABLE scraped_items
            ADD COLUMN IF NOT EXISTS job_id UUID
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            ALTER TABLE scraped_items
            ADD COLUMN IF NOT EXISTS job_inserted_at TIMESTAMPTZ
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS scraped_items_job_id_idx
            ON scraped_items (job_id)
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Seed / refresh known e-rekrutim patterns (API list — no Chromium required)
        let tirana_config = serde_json::json!({
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
        });

        sqlx::query(
            r#"
            INSERT INTO scrape_patterns (id, name, url_match, config, enabled)
            VALUES (
                'a1111111-1111-4111-8111-111111111111',
                'Tirana E-rekrutim',
                'rekrutimi.tirana.al/shpalljet',
                $1::jsonb,
                TRUE
            )
            ON CONFLICT (id) DO UPDATE SET
                config = EXCLUDED.config,
                url_match = EXCLUDED.url_match,
                name = EXCLUDED.name,
                enabled = TRUE,
                updated_at = NOW()
            "#,
        )
        .bind(tirana_config)
        .execute(&self.pool)
        .await?;

        let duapune_config = serde_json::json!({
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
            "detail": {
                "source": "none"
            }
        });

        sqlx::query(
            r#"
            INSERT INTO scrape_patterns (id, name, url_match, config, enabled)
            VALUES (
                'a2222222-2222-4222-8222-222222222222',
                'DuaPune listing',
                'duapune.com',
                $1::jsonb,
                TRUE
            )
            ON CONFLICT (id) DO UPDATE SET
                config = EXCLUDED.config,
                url_match = EXCLUDED.url_match,
                name = EXCLUDED.name,
                enabled = TRUE,
                updated_at = NOW()
            "#,
        )
        .bind(duapune_config)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn save_items(&self, items: &[ScrapedItem], job_id: Uuid) -> Result<u64> {
        let mut saved = 0u64;
        for item in items {
            let description = item.description.as_ref().map(|d| encode_description(d));
            let result = sqlx::query(
                r#"
                INSERT INTO scraped_items
                    (source_id, external_id, item_timestamp, title, url, company, location, salary, description, job_id, job_inserted_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
                ON CONFLICT (source_id, external_id, item_timestamp)
                DO UPDATE SET
                    title = EXCLUDED.title,
                    url = EXCLUDED.url,
                    company = EXCLUDED.company,
                    location = EXCLUDED.location,
                    salary = EXCLUDED.salary,
                    description = EXCLUDED.description,
                    job_id = EXCLUDED.job_id,
                    job_inserted_at = EXCLUDED.job_inserted_at
                "#,
            )
            .bind(&item.source_id)
            .bind(&item.external_id)
            .bind(item.item_timestamp)
            .bind(&item.title)
            .bind(&item.url)
            .bind(&item.company)
            .bind(&item.location)
            .bind(&item.salary)
            .bind(&description)
            .bind(job_id)
            .execute(&self.pool)
            .await?;
            saved += result.rows_affected();
        }
        Ok(saved)
    }

    pub async fn create_job(
        &self,
        listing_url: &str,
        from_date: NaiveDate,
        to_date: NaiveDate,
        pattern_id: Option<Uuid>,
        use_ai: bool,
        ai_provider: Option<&str>,
    ) -> Result<ScrapeJob> {
        let id = Uuid::new_v4();
        let resolved_pattern = if use_ai {
            None
        } else {
            match pattern_id {
                Some(pid) => Some(pid),
                None => self
                    .find_pattern_for_url(listing_url)
                    .await?
                    .map(|p| p.id),
            }
        };
        let provider = if use_ai {
            Some(
                crate::models::AiProvider::parse(ai_provider)
                    .as_str()
                    .to_string(),
            )
        } else {
            None
        };

        let job = sqlx::query_as::<_, ScrapeJob>(
            r#"
            INSERT INTO scrape_jobs (id, listing_url, from_date, to_date, status, pattern_id, use_ai, ai_provider)
            VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(listing_url)
        .bind(from_date)
        .bind(to_date)
        .bind(resolved_pattern)
        .bind(use_ai)
        .bind(provider)
        .fetch_one(&self.pool)
        .await?;
        Ok(job)
    }

    pub async fn list_jobs(&self, limit: i64) -> Result<Vec<ScrapeJob>> {
        let jobs = sqlx::query_as::<_, ScrapeJob>(
            r#"
            SELECT * FROM scrape_jobs
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(jobs)
    }

    pub async fn get_job(&self, id: Uuid) -> Result<Option<ScrapeJob>> {
        let job = sqlx::query_as::<_, ScrapeJob>("SELECT * FROM scrape_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(job)
    }

    pub async fn claim_next_job(&self) -> Result<Option<ScrapeJob>> {
        let job = sqlx::query_as::<_, ScrapeJob>(
            r#"
            UPDATE scrape_jobs
            SET status = 'running', started_at = NOW(), last_error = NULL
            WHERE id = (
                SELECT id FROM scrape_jobs
                WHERE status = 'queued'
                ORDER BY created_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            RETURNING *
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(job)
    }

    pub async fn update_job_progress(
        &self,
        id: Uuid,
        scraped_count: i32,
        pages_visited: i32,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE scrape_jobs
            SET scraped_count = $2, pages_visited = $3
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(scraped_count)
        .bind(pages_visited)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_job(
        &self,
        id: Uuid,
        status: JobStatus,
        scraped_count: i32,
        pages_visited: i32,
        last_error: Option<&str>,
    ) -> Result<ScrapeJob> {
        let job = sqlx::query_as::<_, ScrapeJob>(
            r#"
            UPDATE scrape_jobs
            SET status = $2,
                scraped_count = $3,
                pages_visited = $4,
                last_error = $5,
                finished_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(status.as_str())
        .bind(scraped_count)
        .bind(pages_visited)
        .bind(last_error)
        .fetch_one(&self.pool)
        .await?;
        Ok(job)
    }

    pub async fn stop_job(&self, id: Uuid) -> Result<Option<ScrapeJob>> {
        let job = sqlx::query_as::<_, ScrapeJob>(
            r#"
            UPDATE scrape_jobs
            SET status = 'stopped', finished_at = NOW()
            WHERE id = $1 AND status IN ('queued', 'running')
            RETURNING *
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(job)
    }

    pub async fn queue_depth(&self) -> Result<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrape_jobs WHERE status = 'queued'")
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    pub async fn current_running(&self) -> Result<Option<ScrapeJob>> {
        let job = sqlx::query_as::<_, ScrapeJob>(
            "SELECT * FROM scrape_jobs WHERE status = 'running' ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(job)
    }

    pub async fn list_items(&self, limit: i64) -> Result<Vec<ScrapedItem>> {
        let items = sqlx::query_as::<_, ScrapedItem>(
            r#"
            SELECT * FROM scraped_items
            ORDER BY item_timestamp DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(items)
    }

    pub async fn get_last_watermark(&self, source_id: &str) -> Result<Option<DateTime<Utc>>> {
        let row: (Option<DateTime<Utc>>,) = sqlx::query_as(
            "SELECT MAX(item_timestamp) FROM scraped_items WHERE source_id = $1",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn list_patterns(&self) -> Result<Vec<ScrapePattern>> {
        let rows = sqlx::query_as::<_, ScrapePattern>(
            "SELECT * FROM scrape_patterns ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_pattern(&self, id: Uuid) -> Result<Option<ScrapePattern>> {
        let row = sqlx::query_as::<_, ScrapePattern>("SELECT * FROM scrape_patterns WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn find_pattern_for_url(&self, url: &str) -> Result<Option<ScrapePattern>> {
        let patterns = self.list_patterns().await?;
        let url_l = url.to_lowercase();
        Ok(patterns
            .into_iter()
            .filter(|p| p.enabled)
            .find(|p| url_l.contains(&p.url_match.to_lowercase())))
    }

    pub async fn create_pattern(&self, req: &UpsertPatternRequest) -> Result<ScrapePattern> {
        let id = Uuid::new_v4();
        let config = serde_json::to_value(&req.config).context("serialize pattern config")?;
        let row = sqlx::query_as::<_, ScrapePattern>(
            r#"
            INSERT INTO scrape_patterns (id, name, url_match, config, enabled)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(&req.name)
        .bind(&req.url_match)
        .bind(config)
        .bind(req.enabled.unwrap_or(true))
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn update_pattern(
        &self,
        id: Uuid,
        req: &UpsertPatternRequest,
    ) -> Result<Option<ScrapePattern>> {
        let config = serde_json::to_value(&req.config).context("serialize pattern config")?;
        let row = sqlx::query_as::<_, ScrapePattern>(
            r#"
            UPDATE scrape_patterns
            SET name = $2,
                url_match = $3,
                config = $4,
                enabled = $5,
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(&req.name)
        .bind(&req.url_match)
        .bind(config)
        .bind(req.enabled.unwrap_or(true))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete_pattern(&self, id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM scrape_patterns WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

/// Strip HTML tags, decode entities to Unicode (repeat for double-encoding),
/// collapse whitespace, truncate.
fn encode_description(raw: &str) -> String {
    let stripped = strip_html_tags(raw);
    let decoded = decode_entities_fully(&stripped);
    decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(4000)
        .collect()
}

/// Decode HTML entities until stable. Needed because some sources store
/// double-encoded text (`&amp;euml;` → `&euml;` → `ë`).
fn decode_entities_fully(input: &str) -> String {
    let mut current = input.to_string();
    for _ in 0..5 {
        let next = html_escape::decode_html_entities(&current);
        if next.as_ref() == current.as_str() {
            break;
        }
        current = next.into_owned();
    }
    current
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_description_strips_tags_and_decodes_entities() {
        let raw = "<p>Hello &amp; <b>world</b></p>";
        assert_eq!(encode_description(raw), "Hello & world");
    }

    #[test]
    fn encode_description_decodes_albanian_entities() {
        let raw = "Kryen pun&euml;n specifike t&euml; nj&euml;sis&euml; organizative&nbsp;opsione";
        let encoded = encode_description(raw);
        assert_eq!(
            encoded,
            "Kryen punën specifike të njësisë organizative opsione"
        );
    }

    #[test]
    fn encode_description_decodes_double_encoded_entities() {
        // One decode leaves &euml;; we must decode again.
        let raw = "pun&amp;euml;n";
        assert_eq!(encode_description(raw), "punën");
    }
}
