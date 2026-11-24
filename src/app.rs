use crate::{
    config::Config,
    engine,
    model::{Job, State, now_ms},
    process,
    resources::{Resources, Sampler},
    store::Store,
    telemetry,
};
use anyhow::{Context, Result};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub struct App {
    pub config: Config,
    pub store: Store,
    pub mutation: Mutex<()>,
    pub uploads: Semaphore,
    pub changed: Notify,
    pub shutdown: CancellationToken,
    pub resources: RwLock<Resources>,
    _ownership: File,
}

impl App {
    pub async fn open(config: Config) -> Result<Arc<Self>> {
        let config = config.normalize()?;
        let ownership = process::lock(&config.data_dir.join("api.lock"))?;
        process::wait_guard(&config.data_dir.join("engine.lock")).await?;
        engine::ready(&config).await?;
        let store = Store::open(&config.data_dir.join("jobs.sqlite3"))?;
        store.interrupt_unfinished()?;
        reconcile(&config.data_dir, &store)?;
        Ok(Arc::new(Self {
            config,
            store,
            mutation: Mutex::new(()),
            uploads: Semaphore::new(4),
            changed: Notify::new(),
            shutdown: CancellationToken::new(),
            resources: RwLock::new(Resources::default()),
            _ownership: ownership,
        }))
    }

    pub fn directory(&self, id: Uuid) -> PathBuf {
        self.config.data_dir.join("jobs").join(id.to_string())
    }

    pub fn present(&self, mut job: Job) -> Job {
        let age = job
            .progress
            .sample_at_ms
            .map(|at| now_ms().saturating_sub(at));
        job.progress.fresh = job.state == State::Running && age.is_some_and(|age| age <= 5000);
        if !job.progress.fresh {
            telemetry::stop_live(&mut job.progress);
        }
        telemetry::age_estimates(&mut job.progress, now_ms());
        job.resume_available = job.state.resumable()
            && (job.attempts.is_empty()
                || engine::checkpoint_ready(&self.directory(job.id)).is_ok());
        job.resume_reason = if job.resume_available {
            None
        } else {
            Some(
                if !job.state.resumable() {
                    "Job state cannot be resumed"
                } else {
                    "A valid checkpoint is required; it is missing, oversized or malformed"
                }
                .into(),
            )
        };
        job
    }

    pub async fn sample_resources(self: Arc<Self>) {
        let mut sampler = Sampler::default();
        loop {
            let sample = sampler.sample(
                Path::new("/proc"),
                Path::new("/sys/class/drm"),
                self.config.gpu_pci.as_deref(),
            );
            if let Ok(mut cached) = self.resources.write() {
                *cached = sample;
            }
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                () = tokio::time::sleep(Duration::from_secs(1)) => {},
            }
        }
    }
}

fn reconcile(root: &Path, store: &Store) -> Result<()> {
    // Ownership and engine locks are held before removing abandoned publications.
    for entry in std::fs::read_dir(root.join("staging"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    for entry in std::fs::read_dir(root.join("jobs"))? {
        let entry = entry?;
        let id = entry
            .file_name()
            .to_str()
            .context("Invalid job directory")?
            .parse::<Uuid>()?;
        if store.get(id)?.is_none() {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}
