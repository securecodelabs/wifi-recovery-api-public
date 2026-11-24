use crate::{
    app::App,
    engine,
    error::{ApiError, ApiResult},
    model::{CONTRACT, Job, State},
    pattern, upload,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State as HttpState},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub fn router(app: Arc<App>) -> Router {
    let limit = app.config.max_upload_mib as usize * 1024 * 1024 + 16384;
    Router::new()
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/readyz", get(ready))
        .route("/v1/system", get(system))
        .route("/v1/patterns/preview", post(preview))
        .route("/v1/jobs", get(list).post(upload::submit))
        .route("/v1/jobs/{id}", get(detail).delete(delete))
        .route("/v1/jobs/{id}/stop", post(stop))
        .route("/v1/jobs/{id}/resume", post(resume))
        .route("/v1/jobs/{id}/result", get(result))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "Endpoint not found".into()) })
        .layer(DefaultBodyLimit::max(limit))
        .with_state(app)
}

async fn ready(HttpState(app): HttpState<Arc<App>>) -> ApiResult<Json<Value>> {
    if app.shutdown.is_cancelled() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Shutting down".into(),
        ));
    }
    Ok(Json(
        json!({"contract":CONTRACT,"version":env!("CARGO_PKG_VERSION"),"engine_version":"0.17.2",
        "backend":if app.config.cpu { "cpu" } else { "hip" },"hip_device":app.config.hip_device,"ready":true}),
    ))
}

async fn system(HttpState(app): HttpState<Arc<App>>) -> ApiResult<Json<Value>> {
    let resources = app
        .resources
        .read()
        .map_err(|_| anyhow::anyhow!("Resource cache poisoned"))?;
    Ok(Json(
        json!({"contract":CONTRACT,"sample_age_ms":crate::model::now_ms().saturating_sub(resources.sampled_at_ms),
        "source":"linux_proc_and_amd_sysfs","resources":*resources}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewRequest {
    pattern: String,
}

async fn preview(
    body: Result<Json<PreviewRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<pattern::Preview>> {
    let Json(body) = body.map_err(ApiError::invalid)?;
    Ok(Json(
        pattern::preview(&body.pattern).map_err(ApiError::invalid)?,
    ))
}

#[derive(Default, Deserialize)]
struct Pagination {
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn list(
    HttpState(app): HttpState<Arc<App>>,
    pagination: Result<Query<Pagination>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<Value>> {
    let Query(page) = pagination.map_err(ApiError::invalid)?;
    let limit = page.limit.unwrap_or(50);
    let offset = page.offset.unwrap_or(0);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid("Limit must be 1-100"));
    }
    let (jobs, total) = app.store.list(limit, offset)?;
    let jobs: Vec<_> = jobs.into_iter().map(|job| app.present(job)).collect();
    Ok(Json(
        json!({"jobs":jobs,"total":total,"limit":limit,"offset":offset}),
    ))
}

fn get_job(app: &App, id: &str) -> ApiResult<Job> {
    let id = Uuid::parse_str(id).map_err(ApiError::invalid)?;
    app.store.get(id)?.ok_or_else(ApiError::missing)
}

async fn detail(
    HttpState(app): HttpState<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Job>> {
    Ok(Json(app.present(get_job(&app, &id)?)))
}

async fn stop(HttpState(app): HttpState<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Job>> {
    let _mutation = app.mutation.lock().await;
    let job = get_job(&app, &id)?;
    let job = app.store.update(job.id, |job| {
        job.state = match job.state {
            State::Queued => State::Stopped,
            State::Running => State::Stopping,
            state => state,
        };
        Ok(())
    })?;
    app.changed.notify_one();
    Ok(Json(app.present(job)))
}

async fn resume(
    HttpState(app): HttpState<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Job>> {
    let _mutation = app.mutation.lock().await;
    let job = get_job(&app, &id)?;
    if job.state == State::Queued {
        return Ok(Json(app.present(job)));
    }
    if !job.state.resumable() {
        return Err(ApiError::conflict("Job state cannot be resumed"));
    }
    if !job.attempts.is_empty() {
        engine::checkpoint_ready(&app.directory(job.id)).map_err(ApiError::conflict)?;
    }
    let job = app.store.enqueue(job.id)?;
    app.changed.notify_one();
    Ok(Json(app.present(job)))
}

async fn result(
    HttpState(app): HttpState<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _mutation = app.mutation.lock().await;
    let job = get_job(&app, &id)?;
    if job.state.active() {
        return Err(ApiError::conflict(
            "Stop or finish the job before retrieving results",
        ));
    }
    let value = engine::document(&app.directory(job.id).join("result.json"))
        .map_err(|error| ApiError::conflict(format!("Result unavailable: {error}")))?;
    engine::validate_result(&value).map_err(ApiError::conflict)?;
    Ok(Json(
        json!({"contract":CONTRACT,"job_id":job.id,"state":job.state,"engine_result":value}),
    ))
}

async fn delete(
    HttpState(app): HttpState<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let _mutation = app.mutation.lock().await;
    let job = get_job(&app, &id)?;
    if job.state.active() {
        return Err(ApiError::conflict("Stop the job before deleting it"));
    }
    app.store.delete(job.id)?;
    std::fs::remove_dir_all(app.directory(job.id)).map_err(anyhow::Error::from)?;
    upload::sync_directory(&app.config.data_dir.join("jobs"))?;
    Ok(StatusCode::NO_CONTENT)
}
