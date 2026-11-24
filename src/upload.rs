use crate::{
    app::App,
    error::{ApiError, ApiResult},
    model::{Job, Progress, Search, State, now_ms},
    pattern, process,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json,
    extract::{Multipart, State as HttpState},
    http::{HeaderMap, StatusCode},
};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

struct Upload {
    staging: TempDir,
    search: Search,
    filename: String,
    bytes: u64,
    digest: String,
    gpu_batch: u32,
}

pub async fn submit(
    HttpState(app): HttpState<Arc<App>>,
    headers: HeaderMap,
    body: Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> ApiResult<(StatusCode, HeaderMap, Json<Job>)> {
    let body = body.map_err(ApiError::invalid)?;
    let _slot = app.uploads.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Upload capacity reached".into(),
        )
    })?;
    let key = headers
        .get("Idempotency-Key")
        .map(|value| {
            value
                .to_str()
                .map_err(ApiError::invalid)
                .and_then(|text| Uuid::parse_str(text).map_err(ApiError::invalid))
        })
        .transpose()?;
    let upload = tokio::time::timeout(Duration::from_secs(60), receive(&app, body))
        .await
        .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "Upload timed out".into()))?
        .map_err(ApiError::invalid)?;
    let settings = serde_json::to_vec(&upload.search).map_err(anyhow::Error::from)?;
    let fingerprint = hex::encode(Sha256::digest(
        [upload.digest.as_bytes(), &settings].concat(),
    ));
    let _mutation = app.mutation.lock().await;
    if let Some(job) = key
        .map(|key| app.store.replay(key, &fingerprint))
        .transpose()
        .map_err(ApiError::conflict)?
        .flatten()
    {
        return response(&app, StatusCode::OK, job);
    }
    let (_, count) = app.store.list(0, 0)?;
    if count >= u64::from(app.config.max_jobs) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Job storage is full".into(),
        ));
    }
    let job = make_job(&upload);
    let directory = app.directory(job.id);
    std::fs::rename(upload.staging.path(), &directory).map_err(anyhow::Error::from)?;
    sync_directory(directory.parent().context("Missing jobs parent")?)?;
    if let Err(error) = app.store.insert(
        &job,
        key.map(|key| (key, fingerprint.as_str())),
        app.config.max_jobs,
    ) {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error.into());
    }
    app.changed.notify_one();
    response(&app, StatusCode::CREATED, job)
}

fn response(
    app: &App,
    status: StatusCode,
    job: Job,
) -> ApiResult<(StatusCode, HeaderMap, Json<Job>)> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Location",
        format!("/v1/jobs/{}", job.id)
            .parse()
            .map_err(anyhow::Error::from)?,
    );
    Ok((status, headers, Json(app.present(job))))
}

async fn receive(app: &App, mut body: Multipart) -> Result<Upload> {
    let staging = tempfile::tempdir_in(app.config.data_dir.join("staging"))?;
    let mut search = None;
    let mut uploaded = None;
    while let Some(mut field) = body.next_field().await? {
        match field.name().context("Multipart field requires a name")? {
            "metadata" if search.is_none() => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field.chunk().await? {
                    ensure!(
                        bytes.len() + chunk.len() <= 4096,
                        "Metadata exceeds 4096 bytes"
                    );
                    bytes.extend_from_slice(&chunk);
                }
                let value: Search = serde_json::from_slice(&bytes)?;
                pattern::preview(&value.pattern)?;
                search = Some(value);
            }
            "file" if uploaded.is_none() => {
                uploaded = Some(
                    write_file(
                        field,
                        staging.path(),
                        u64::from(app.config.max_upload_mib) * 1024 * 1024,
                    )
                    .await?,
                );
            }
            _ => anyhow::bail!("Expected exactly one metadata field and one file field"),
        }
    }
    let search = search.context("Missing metadata field")?;
    let (filename, bytes, digest) = uploaded.context("Missing file field")?;
    let gpu_batch = inspect(app, &staging.path().join("input.hc22000")).await?;
    sync_directory(staging.path())?;
    Ok(Upload {
        staging,
        search,
        filename,
        bytes,
        digest,
        gpu_batch,
    })
}

async fn write_file(
    mut field: axum::extract::multipart::Field<'_>,
    directory: &Path,
    limit: u64,
) -> Result<(String, u64, String)> {
    let filename = field
        .file_name()
        .unwrap_or("input.hc22000")
        .chars()
        .take(256)
        .collect();
    let mut file = tokio::fs::File::create(directory.join("input.hc22000")).await?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    while let Some(chunk) = field.chunk().await? {
        length += chunk.len() as u64;
        ensure!(length <= limit, "HC22000 upload exceeded size limit");
        digest.update(&chunk);
        file.write_all(&chunk).await?;
    }
    ensure!(length > 0, "HC22000 input is empty");
    file.sync_all().await?;
    Ok((filename, length, hex::encode(digest.finalize())))
}

async fn inspect(app: &App, input: &Path) -> Result<u32> {
    let output = process::probe(
        &app.config.engine,
        &[
            "inspect".into(),
            "--hashes".into(),
            input.as_os_str().to_owned(),
        ],
        Duration::from_secs(15),
    )
    .await?;
    let records: u64 = output
        .split_whitespace()
        .find_map(|part| part.strip_prefix("records="))
        .context("Engine did not report record count")?
        .parse()?;
    ensure!(
        (1..=10000).contains(&records),
        "Input must contain 1-10000 distinct records"
    );
    Ok(app
        .config
        .gpu_batch
        .unwrap_or_else(|| automatic_batch(&output, records)))
}

fn automatic_batch(inspection: &str, records: u64) -> u32 {
    let exact = inspection
        .lines()
        .find(|line| line.starts_with("line="))
        .is_some_and(|line| {
            ["type=2", "key_version=2", "nonce_variants=1"]
                .iter()
                .all(|expected| line.split_whitespace().any(|field| field == *expected))
        });
    if records == 1 && exact { 262144 } else { 4096 }
}

fn make_job(upload: &Upload) -> Job {
    let now = now_ms();
    Job {
        id: Uuid::new_v4(),
        state: State::Queued,
        search: upload.search.clone(),
        gpu_batch: upload.gpu_batch,
        filename: upload.filename.clone(),
        input_sha256: upload.digest.clone(),
        input_bytes: upload.bytes,
        created_at_ms: now,
        updated_at_ms: now,
        first_started_at_ms: None,
        total_active_seconds: 0.0,
        attempts: Vec::new(),
        progress: Progress::default(),
        error: None,
        resume_available: false,
        resume_reason: None,
    }
}

pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
