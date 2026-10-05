use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScrapedItem {
    pub source_id: String,
    pub external_id: String,
    pub item_timestamp: DateTime<Utc>,
    pub title: String,
    pub url: String,
    pub company: Option<String>,
    pub location: Option<String>,
    pub salary: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Stopped,
    Completed,
    Failed,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "stopped" => Self::Stopped,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Queued,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScrapeJob {
    pub id: Uuid,
    pub listing_url: String,
    pub from_date: NaiveDate,
    pub to_date: NaiveDate,
    pub status: String,
    pub scraped_count: i32,
    pub pages_visited: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub url: String,
    pub from_date: NaiveDate,
    pub to_date: NaiveDate,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub current_job: Option<ScrapeJob>,
    pub queue_depth: i64,
    pub scraped_count: i32,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
pub struct ScrapeStats {
    pub scraped_count: i32,
    pub pages_visited: i32,
}
