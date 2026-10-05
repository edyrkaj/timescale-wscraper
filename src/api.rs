use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures_util::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::models::{CreateJobRequest, ProgressEvent};
use crate::AppState;

pub async fn create_job(
    State(state): State<AppState>,
    Json(body): Json<CreateJobRequest>,
) -> impl IntoResponse {
    if body.from_date > body.to_date {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "from_date must be <= to_date"})),
        )
            .into_response();
    }
    if url::Url::parse(&body.url).is_err() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid url"})),
        )
            .into_response();
    }

    match state
        .db
        .create_job(&body.url, body.from_date, body.to_date)
        .await
    {
        Ok(job) => {
            state.queue.notify();
            let _ = state.events.send(ProgressEvent {
                current_job: state.db.current_running().await.ok().flatten(),
                queue_depth: state.db.queue_depth().await.unwrap_or(0),
                scraped_count: 0,
                status: "queued".into(),
                message: format!("Enqueued job {}", job.id),
            });
            (axum::http::StatusCode::CREATED, Json(job)).into_response()
        }
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

pub async fn list_jobs(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.list_jobs(50).await {
        Ok(jobs) => Json(jobs).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

pub async fn stop_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.queue.cancel_job(id).await {
        Ok(()) => {
            let job = state.db.get_job(id).await.ok().flatten();
            Json(serde_json::json!({"ok": true, "job": job})).into_response()
        }
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

pub async fn restart_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.db.get_job(id).await {
        Ok(Some(job)) => match state
            .db
            .create_job(&job.listing_url, job.from_date, job.to_date)
            .await
        {
            Ok(new_job) => {
                state.queue.notify();
                (axum::http::StatusCode::CREATED, Json(new_job)).into_response()
            }
            Err(err) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response(),
        },
        Ok(None) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "job not found"})),
        )
            .into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

pub async fn list_items(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.list_items(100).await {
        Ok(items) => Json(items).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

pub async fn job_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();

    let current = state.db.current_running().await.ok().flatten();
    let scraped_count = current.as_ref().map(|j| j.scraped_count).unwrap_or(0);
    let status = current
        .as_ref()
        .map(|j| j.status.clone())
        .unwrap_or_else(|| "idle".into());
    let snapshot = ProgressEvent {
        current_job: current,
        queue_depth: state.db.queue_depth().await.unwrap_or(0),
        scraped_count,
        status,
        message: "connected".into(),
    };
    let initial = futures_util::stream::once(async move {
        Ok::<Event, Infallible>(
            Event::default()
                .event("progress")
                .data(serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".into())),
        )
    });

    let live = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(event) => Some(Ok(Event::default()
            .event("progress")
            .data(serde_json::to_string(&event).unwrap_or_else(|_| "{}".into())))),
        Err(_) => None,
    });

    Sse::new(initial.chain(live)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
