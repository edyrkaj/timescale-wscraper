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
    pub pattern_id: Option<Uuid>,
    #[serde(default)]
    pub use_ai: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub url: String,
    pub from_date: NaiveDate,
    pub to_date: NaiveDate,
    pub pattern_id: Option<Uuid>,
    #[serde(default)]
    pub use_ai: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScrapePattern {
    pub id: Uuid,
    pub name: String,
    pub url_match: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertPatternRequest {
    pub name: String,
    pub url_match: String,
    pub config: PatternConfig,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternConfig {
    pub list: ListConfig,
    #[serde(default)]
    pub detail: Option<DetailConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListConfig {
    pub source: ListSource,
    #[serde(default)]
    pub api: Option<ApiListConfig>,
    #[serde(default)]
    pub dom: Option<DomListConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListSource {
    Api,
    Dom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiListConfig {
    pub url_template: String,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default = "default_root_path")]
    pub items_path: String,
    pub id_path: String,
    #[serde(default)]
    pub published_at_path: Option<String>,
    #[serde(default)]
    pub title_path: Option<String>,
    #[serde(default)]
    pub company_path: Option<String>,
    #[serde(default)]
    pub company_literal: Option<String>,
    #[serde(default)]
    pub location_path: Option<String>,
    #[serde(default)]
    pub salary_path: Option<String>,
    #[serde(default)]
    pub description_path: Option<String>,
    #[serde(default)]
    pub detail_url_template: Option<String>,
}

fn default_page_size() -> u32 {
    50
}

fn default_root_path() -> String {
    "$".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomListConfig {
    #[serde(default)]
    pub wait_for: Option<String>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
    /// CSS selector for each job card on the listing page
    #[serde(default)]
    pub card_selector: Option<String>,
    /// Within a card: job title + detail link (preferred over item_link_selector)
    #[serde(default)]
    pub title_selector: Option<String>,
    #[serde(default)]
    pub company_selector: Option<String>,
    #[serde(default)]
    pub location_selector: Option<String>,
    #[serde(default)]
    pub date_selector: Option<String>,
    /// CSS selector for anchors that point to detail pages (link-only mode)
    #[serde(default)]
    pub item_link_selector: Option<String>,
    /// Regex applied to hrefs if selector not enough
    #[serde(default)]
    pub item_link_regex: Option<String>,
    #[serde(default)]
    pub next_page_selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailConfig {
    pub source: DetailSource,
    #[serde(default)]
    pub wait_ms: Option<u64>,
    /// e.g. https://host/api/api/Job/get-positions-by-job/{id}
    #[serde(default)]
    pub api_url_template: Option<String>,
    #[serde(default)]
    pub description_path: Option<String>,
    #[serde(default)]
    pub fields: DetailFields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailSource {
    Page,
    Api,
    None,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetailFields {
    #[serde(default)]
    pub title_selector: Option<String>,
    #[serde(default)]
    pub title_label: Option<String>,
    #[serde(default)]
    pub company_selector: Option<String>,
    #[serde(default)]
    pub company_label: Option<String>,
    #[serde(default)]
    pub location_selector: Option<String>,
    #[serde(default)]
    pub location_label: Option<String>,
    #[serde(default)]
    pub salary_selector: Option<String>,
    #[serde(default)]
    pub salary_label: Option<String>,
    #[serde(default)]
    pub description_selector: Option<String>,
    #[serde(default)]
    pub date_selector: Option<String>,
    #[serde(default)]
    pub date_label: Option<String>,
    #[serde(default)]
    pub date_regex: Option<String>,
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
