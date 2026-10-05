mod api;
mod db;
mod models;
mod queue;
mod scraper;

use axum::routing::{get, post, put};
use axum::Router;
use db::Db;
use queue::QueueHandle;
use scraper::BrowserPool;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::models::ProgressEvent;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub queue: QueueHandle,
    pub events: broadcast::Sender<ProgressEvent>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://postgres:password@localhost:5432/scrapers_db".to_string()
    });
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    info!("connecting to database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    let db = Db::new(pool);
    db.ensure_schema().await?;

    info!("launching Chromium (optional for API patterns)");
    let browser = match BrowserPool::try_launch().await {
        Ok(pool) => Some(Arc::new(pool)),
        Err(err) => {
            tracing::warn!(error = %err, "Chromium unavailable; API-based patterns will still work");
            None
        }
    };

    let (events, _) = broadcast::channel::<ProgressEvent>(256);
    let queue = QueueHandle::new(db.clone(), browser, events.clone());
    queue.clone().spawn();

    let state = AppState {
        db,
        queue,
        events,
    };

    let static_dir = resolve_static_dir();
    let index = static_dir.join("index.html");

    let app = Router::new()
        .route("/api/jobs", post(api::create_job).get(api::list_jobs))
        .route("/api/jobs/stream", get(api::job_stream))
        .route("/api/jobs/{id}/stop", post(api::stop_job))
        .route("/api/jobs/{id}/restart", post(api::restart_job))
        .route("/api/items", get(api::list_items))
        .route(
            "/api/patterns",
            get(api::list_patterns).post(api::create_pattern),
        )
        .route(
            "/api/patterns/{id}",
            put(api::update_pattern).delete(api::delete_pattern),
        )
        .fallback_service(
            ServeDir::new(&static_dir).not_found_service(ServeFile::new(index)),
        )
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr: SocketAddr = bind_addr.parse()?;
    info!("listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn resolve_static_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("static"),
        PathBuf::from("/app/static"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("static")))
            .unwrap_or_else(|| PathBuf::from("static")),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("index.html").exists())
        .unwrap_or_else(|| PathBuf::from("static"))
}
