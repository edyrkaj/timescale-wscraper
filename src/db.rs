use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{JobStatus, ScrapeJob, ScrapedItem};

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn save_items(&self, items: &[ScrapedItem]) -> Result<u64> {
        let mut saved = 0u64;
        for item in items {
            let result = sqlx::query(
                r#"
                INSERT INTO scraped_items
                    (source_id, external_id, item_timestamp, title, url, company, location, salary, description)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (source_id, external_id, item_timestamp)
                DO UPDATE SET
                    title = EXCLUDED.title,
                    url = EXCLUDED.url,
                    company = EXCLUDED.company,
                    location = EXCLUDED.location,
                    salary = EXCLUDED.salary,
                    description = EXCLUDED.description
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
            .bind(&item.description)
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
    ) -> Result<ScrapeJob> {
        let id = Uuid::new_v4();
        let job = sqlx::query_as::<_, ScrapeJob>(
            r#"
            INSERT INTO scrape_jobs (id, listing_url, from_date, to_date, status)
            VALUES ($1, $2, $3, $4, 'queued')
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(listing_url)
        .bind(from_date)
        .bind(to_date)
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
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrape_jobs WHERE status = 'queued'",
        )
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
}
