use crate::model::{Attempt, Job, Progress, State, now_ms};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;

pub struct Store(Mutex<Connection>);

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY, state TEXT NOT NULL, queue INTEGER NOT NULL, document TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS attempts (
                job TEXT NOT NULL, number INTEGER NOT NULL, document TEXT NOT NULL,
                PRIMARY KEY(job, number));
            CREATE TABLE IF NOT EXISTS requests (
                key TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, job TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS sequence (value INTEGER NOT NULL);
            INSERT INTO sequence SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM sequence);")?;
        Ok(Self(Mutex::new(connection)))
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock poisoned"))
    }

    pub fn get(&self, id: Uuid) -> Result<Option<Job>> {
        load(&*self.connection()?, &id.to_string())
    }

    pub fn list(&self, limit: u32, offset: u32) -> Result<(Vec<Job>, u64)> {
        let db = self.connection()?;
        let total: u32 = db.query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))?;
        let mut query =
            db.prepare("SELECT id FROM jobs ORDER BY queue DESC,id LIMIT ?1 OFFSET ?2")?;
        let ids = query.query_map(params![limit, offset], |row| row.get::<_, String>(0))?;
        let jobs = ids
            .map(|id| load(&db, &id?)?.context("Listed job disappeared"))
            .collect::<Result<_>>()?;
        Ok((jobs, u64::from(total)))
    }

    pub fn replay(&self, key: Uuid, fingerprint: &str) -> Result<Option<Job>> {
        let db = self.connection()?;
        let saved: Option<(String, String)> = db
            .query_row(
                "SELECT fingerprint,job FROM requests WHERE key=?1",
                [key.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((original, id)) = saved else {
            return Ok(None);
        };
        ensure!(
            original == fingerprint,
            "Idempotency key belongs to different input or settings"
        );
        Ok(Some(
            load(&db, &id)?.context("Idempotency key belongs to a deleted job")?,
        ))
    }

    pub fn insert(&self, job: &Job, request: Option<(Uuid, &str)>, capacity: u32) -> Result<()> {
        let mut db = self.connection()?;
        let tx = db.transaction()?;
        let count: u32 = tx.query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))?;
        ensure!(count < capacity, "Job storage is full");
        let order = next_order(&tx)?;
        tx.execute(
            "INSERT INTO jobs VALUES(?1,?2,?3,?4)",
            params![
                job.id.to_string(),
                state_name(job.state)?,
                order,
                serde_json::to_string(job)?
            ],
        )?;
        if let Some((key, fingerprint)) = request {
            tx.execute(
                "INSERT INTO requests VALUES(?1,?2,?3)",
                params![key.to_string(), fingerprint, job.id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update(&self, id: Uuid, action: impl FnOnce(&mut Job) -> Result<()>) -> Result<Job> {
        let mut db = self.connection()?;
        let tx = db.transaction()?;
        let mut job = load(&tx, &id.to_string())?.context("Job not found")?;
        action(&mut job)?;
        job.updated_at_ms = now_ms();
        save(&tx, &job)?;
        tx.commit()?;
        Ok(job)
    }

    pub fn claim(&self) -> Result<Option<Job>> {
        let mut db = self.connection()?;
        let tx = db.transaction()?;
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM jobs WHERE state='queued' ORDER BY queue,id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        let mut job = load(&tx, &id)?.context("Claimed job disappeared")?;
        let started = now_ms();
        job.state = State::Running;
        job.first_started_at_ms.get_or_insert(started);
        job.updated_at_ms = started;
        job.error = None;
        job.resume_available = false;
        job.progress = Progress {
            stage: "preparing".into(),
            ..Progress::default()
        };
        job.attempts.push(Attempt {
            number: job.attempts.len() as u32 + 1,
            started_at_ms: started,
            finished_at_ms: None,
            search_started_at_ms: None,
            elapsed_seconds: 0.0,
            timing_complete: false,
            outcome: State::Running,
            engine_run_at_ms: None,
            error: None,
        });
        save(&tx, &job)?;
        tx.commit()?;
        Ok(Some(job))
    }

    pub fn enqueue(&self, id: Uuid) -> Result<Job> {
        let mut db = self.connection()?;
        let tx = db.transaction()?;
        let mut job = load(&tx, &id.to_string())?.context("Job not found")?;
        if job.state == State::Queued {
            return Ok(job);
        }
        ensure!(job.state.resumable(), "Job state cannot be resumed");
        job.state = State::Queued;
        job.updated_at_ms = now_ms();
        job.progress.fresh = false;
        job.error = None;
        let order = next_order(&tx)?;
        tx.execute(
            "UPDATE jobs SET queue=?1 WHERE id=?2",
            params![order, id.to_string()],
        )?;
        save(&tx, &job)?;
        tx.commit()?;
        Ok(job)
    }

    pub fn delete(&self, id: Uuid) -> Result<()> {
        let mut db = self.connection()?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM attempts WHERE job=?1", [id.to_string()])?;
        tx.execute("DELETE FROM jobs WHERE id=?1", [id.to_string()])?;
        // Request tombstones prevent a delayed retry recreating a deleted search.
        tx.commit()?;
        Ok(())
    }

    pub fn interrupt_unfinished(&self) -> Result<()> {
        let (jobs, _) = self.list(u32::MAX, 0)?;
        for job in jobs {
            if matches!(job.state, State::Running | State::Stopping) {
                self.update(job.id, mark_interrupted)?;
            }
        }
        Ok(())
    }
}

fn mark_interrupted(job: &mut Job) -> Result<()> {
    job.state = State::Interrupted;
    job.error = Some("API stopped during this attempt; timing is last-known".into());
    crate::telemetry::stop_live(&mut job.progress);
    if let Some(attempt) = job.attempts.last_mut() {
        attempt.outcome = State::Interrupted;
        attempt.timing_complete = false;
        attempt.error = job.error.clone();
    }
    Ok(())
}

fn load(db: &Connection, id: &str) -> Result<Option<Job>> {
    let text: Option<String> = db
        .query_row("SELECT document FROM jobs WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    let Some(text) = text else { return Ok(None) };
    let mut job: Job = serde_json::from_str(&text)?;
    let mut query = db.prepare("SELECT document FROM attempts WHERE job=?1 ORDER BY number")?;
    job.attempts = query
        .query_map([id], |r| r.get::<_, String>(0))?
        .map(|text| Ok(serde_json::from_str(&text?)?))
        .collect::<Result<_>>()?;
    Ok(Some(job))
}

fn save(tx: &Transaction<'_>, job: &Job) -> Result<()> {
    let mut document = job.clone();
    document.attempts.clear();
    tx.execute(
        "UPDATE jobs SET state=?1,document=?2 WHERE id=?3",
        params![
            state_name(job.state)?,
            serde_json::to_string(&document)?,
            job.id.to_string()
        ],
    )?;
    for attempt in &job.attempts {
        tx.execute(
            "INSERT OR REPLACE INTO attempts VALUES(?1,?2,?3)",
            params![
                job.id.to_string(),
                attempt.number,
                serde_json::to_string(attempt)?
            ],
        )?;
    }
    Ok(())
}

fn next_order(tx: &Transaction<'_>) -> Result<i64> {
    tx.execute("UPDATE sequence SET value=value+1", [])?;
    Ok(tx.query_row("SELECT value FROM sequence", [], |row| row.get(0))?)
}

fn state_name(state: State) -> Result<String> {
    match serde_json::to_value(state)? {
        serde_json::Value::String(name) => Ok(name),
        _ => bail!("State encoding is not a string"),
    }
}
