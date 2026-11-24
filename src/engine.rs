use crate::{
    config::Config,
    model::{Job, State},
    process,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{ffi::OsString, io::Read, path::Path, time::Duration};

pub const DOCUMENT_LIMIT: u64 = 32 * 1024 * 1024;

pub async fn ready(config: &Config) -> Result<()> {
    let version = process::probe(
        &config.engine,
        &["--version".into()],
        Duration::from_secs(10),
    )
    .await?;
    ensure!(
        version.split_whitespace().any(|part| part == "0.17.2"),
        "WiFi GPU 0.17.2 is required: {version}"
    );
    if !config.cpu {
        let devices = process::probe(
            &config.engine,
            &["devices".into(), "--backend".into(), "hip".into()],
            Duration::from_secs(30),
        )
        .await?;
        ensure!(
            devices.lines().any(
                |line| line.starts_with(&format!("Device {}:", config.hip_device))
                    || line.starts_with(&format!("[{}]", config.hip_device))
            ),
            "Configured HIP device is unavailable: {devices}"
        );
    }
    Ok(())
}

pub fn arguments(config: &Config, job: &Job, dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["recover", "--hashes"]
        .into_iter()
        .map(Into::into)
        .collect();
    args.push(dir.join("input.hc22000").into_os_string());
    args.extend([
        "--mask".into(),
        job.search.pattern.clone().into(),
        "--runtime".into(),
        job.search.runtime_seconds.to_string().into(),
        "--limit".into(),
        job.search.candidate_limit.to_string().into(),
        "--status-every".into(),
        "1".into(),
        "--activity-json".into(),
        "--output".into(),
        dir.join("result.json").into_os_string(),
    ]);
    args.push(
        if job.attempts.len() > 1 {
            "--resume"
        } else {
            "--session"
        }
        .into(),
    );
    args.push(dir.join("checkpoint").into_os_string());
    if config.cpu {
        args.extend(["--cpu".into(), "--threads".into(), "2".into()]);
    } else {
        args.extend([
            "--backend".into(),
            "hip".into(),
            "--device".into(),
            config.hip_device.to_string().into(),
            "--batch".into(),
            job.gpu_batch.to_string().into(),
        ]);
    }
    args
}

pub fn document(path: &Path) -> Result<Value> {
    let file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= DOCUMENT_LIMIT,
        "Engine document exceeded limit"
    );
    let mut bytes = Vec::new();
    file.take(DOCUMENT_LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= DOCUMENT_LIMIT,
        "Engine document exceeded limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn checkpoint_ready(dir: &Path) -> Result<()> {
    let path = dir.join("checkpoint");
    let size = std::fs::metadata(&path)?.len();
    ensure!(
        (1..=DOCUMENT_LIMIT).contains(&size),
        "Checkpoint is missing or oversized"
    );
    let text = std::fs::read_to_string(path)?;
    let (payload, checksum) = text
        .rsplit_once("sha256\t")
        .context("Missing checkpoint checksum")?;
    ensure!(
        checksum == format!("{}\n", hex::encode(Sha256::digest(payload.as_bytes()))),
        "Checkpoint checksum failed"
    );
    let mut lines = payload.lines();
    ensure!(
        lines.next() == Some("WIFI-GPU-SESSION\t1"),
        "Unsupported checkpoint format"
    );
    let identity = lines
        .next()
        .and_then(|line| line.strip_prefix("job\t"))
        .context("Missing checkpoint identity")?;
    ensure!(
        identity.len() == 64 && hex::decode(identity).is_ok(),
        "Invalid checkpoint identity"
    );
    lines
        .next()
        .and_then(|line| line.strip_prefix("next\t"))
        .context("Missing checkpoint offset")?
        .parse::<u64>()?;
    if let Ok(result) = document(&dir.join("result.json")) {
        ensure!(
            result["job_id"] == identity,
            "Checkpoint belongs to different engine input"
        );
    }
    // The engine independently verifies saved matches and the immutable input fingerprint.
    Ok(())
}

pub fn validate_result(value: &Value) -> Result<()> {
    ensure!(value["schema"] == 1, "Unsupported engine result schema");
    ensure!(value["version"] == "0.17.2", "Unexpected engine version");
    let total = value["total_records"]
        .as_u64()
        .context("Missing record count")?;
    let recovered = value["recovered"]
        .as_u64()
        .context("Missing recovered count")?;
    let matches = value["matches"]
        .as_array()
        .context("Missing verified matches")?;
    ensure!(
        total > 0 && total <= 10000 && recovered <= total && recovered == matches.len() as u64,
        "Invalid result counts"
    );
    for item in matches {
        let password = item["password_hex"]
            .as_str()
            .context("Missing password bytes")?;
        let bytes = hex::decode(password)?;
        ensure!(
            item["verified"] == true && (8..=63).contains(&bytes.len()),
            "Unverified or invalid result"
        );
    }
    Ok(())
}

pub fn outcome(code: Option<i32>, value: &Value) -> Result<State> {
    validate_result(value)?;
    let status = value["status"].as_str().context("Missing result status")?;
    match (code, status) {
        (Some(0), "recovered") if value["recovered"] == value["total_records"] => {
            Ok(State::Recovered)
        }
        (Some(2), "exhausted") => Ok(State::Exhausted),
        (Some(3), "interrupted") => Ok(State::Stopped),
        (Some(4), "runtime_limit" | "candidate_limit") => Ok(State::LimitReached),
        _ => bail!("Engine exit {code:?} disagrees with result status {status}"),
    }
}
