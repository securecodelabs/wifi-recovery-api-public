use crate::{
    app::App,
    engine,
    model::{Job, State, now_ms},
    telemetry::{Observation, stop_live},
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
};

pub async fn run(app: Arc<App>) -> Result<()> {
    loop {
        if app.shutdown.is_cancelled() {
            return Ok(());
        }
        let claimed = {
            let _mutation = app.mutation.lock().await;
            app.store.claim()?
        };
        if let Some(job) = claimed {
            if let Err(error) = execute(&app, &job).await {
                let _mutation = app.mutation.lock().await;
                app.store
                    .update(job.id, |saved| mark_failed(saved, error))?;
            }
            continue;
        }
        tokio::select! { () = app.changed.notified() => {}, () = app.shutdown.cancelled() => return Ok(()) }
    }
}

fn mark_failed(job: &mut Job, error: anyhow::Error) -> Result<()> {
    job.state = State::Failed;
    job.error = Some(format!("{error:#}"));
    stop_live(&mut job.progress);
    if let Some(attempt) = job.attempts.last_mut() {
        attempt.outcome = State::Failed;
        attempt.finished_at_ms = Some(now_ms());
        attempt.error = job.error.clone();
    }
    Ok(())
}

struct Run {
    child: Child,
    parent_pipe: Option<ChildStdin>,
    events: mpsc::Receiver<(bool, String)>,
    drains: Vec<tokio::task::JoinHandle<Result<()>>>,
    observation: Observation,
    last_document: Option<Value>,
    clock: Instant,
    base_elapsed: f64,
}

async fn execute(app: &App, job: &Job) -> Result<()> {
    let directory = app.directory(job.id);
    let previous = engine::document(&directory.join("result.json")).ok();
    let mut run = spawn(app, job, &directory).await?;
    run.observation.previous_run = previous
        .as_ref()
        .and_then(|v| v["telemetry"]["run_started_at_ms"].as_u64());
    run.observation.start_offset = previous
        .as_ref()
        .and_then(|v| v["next_candidate"].as_u64())
        .unwrap_or(0);
    let result = monitor(app, job, &directory, &mut run).await;
    run.parent_pipe.take();
    let waited = run.child.wait().await;
    let mut completed = result.and(waited.map_err(Into::into));
    for drain in std::mem::take(&mut run.drains) {
        if let Err(error) = drain
            .await
            .context("Engine output task failed")
            .and_then(|result| result)
        {
            completed = Err(error);
        }
    }
    while let Ok((stdout, line)) = run.events.try_recv() {
        observe(&mut run.observation, stdout, &line);
    }
    update_snapshot(
        &directory,
        job,
        &mut run.observation,
        &mut run.last_document,
    );
    let outcome = completed.and_then(|status| {
        let value = run
            .last_document
            .as_ref()
            .context("Engine produced no result for this attempt")?;
        engine::outcome(status.code(), value)
    });
    let state = outcome.as_ref().copied().unwrap_or(State::Failed);
    let _mutation = app.mutation.lock().await;
    persist(
        app,
        job,
        &run.observation,
        run.clock.elapsed().as_secs_f64(),
        run.base_elapsed,
        Some(state),
    )?;
    if let Err(error) = outcome {
        app.store
            .update(job.id, |saved| mark_failed(saved, error))?;
    }
    Ok(())
}

async fn spawn(app: &App, job: &Job, directory: &Path) -> Result<Run> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("__engine-guard")
        .arg(app.config.data_dir.join("engine.lock"))
        .arg(std::process::id().to_string())
        .arg(&app.config.engine)
        .args(engine::arguments(&app.config, job, directory))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let parent_pipe = child.stdin.take();
    let stdout = child.stdout.take().context("Missing engine stdout")?;
    let stderr = child.stderr.take().context("Missing engine stderr")?;
    let (tx, events) = mpsc::channel(128);
    let number = job.attempts.len();
    let drains = vec![
        tokio::spawn(drain(
            stdout,
            true,
            tx.clone(),
            directory.join(format!("attempt-{number}.stdout")),
        )),
        tokio::spawn(drain(
            stderr,
            false,
            tx,
            directory.join(format!("attempt-{number}.stderr")),
        )),
    ];
    Ok(Run {
        child,
        parent_pipe,
        events,
        drains,
        observation: Observation::default(),
        last_document: None,
        clock: Instant::now(),
        base_elapsed: job.total_active_seconds,
    })
}

async fn monitor(
    app: &App,
    job: &Job,
    directory: &Path,
    run: &mut Run,
) -> Result<std::process::ExitStatus> {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            event = run.events.recv() => {
                if let Some((stdout, line)) = event { observe(&mut run.observation, stdout, &line); }
                else { return Ok(run.child.wait().await?); }
            }
            _ = interval.tick() => {
                if let Some(status) = run.child.try_wait()? { return Ok(status); }
                let _mutation = app.mutation.lock().await;
                let state = app.store.get(job.id)?.context("Running job disappeared")?.state;
                let stopping = state == State::Stopping || app.shutdown.is_cancelled();
                let signal_ready = !run.observation.progress.stage.is_empty() || run.clock.elapsed() >= Duration::from_secs(15);
                if stopping && signal_ready { run.parent_pipe.take(); }
                update_snapshot(directory, job, &mut run.observation, &mut run.last_document);
                persist(app, job, &run.observation, run.clock.elapsed().as_secs_f64(), run.base_elapsed, None)?;
            }
        }
    }
}

fn update_snapshot(
    directory: &Path,
    job: &Job,
    observation: &mut Observation,
    previous: &mut Option<Value>,
) {
    let Ok(value) = engine::document(&directory.join("result.json")) else {
        return;
    };
    if previous.as_ref() == Some(&value)
        || value["telemetry"]["run_started_at_ms"].as_u64() == observation.previous_run
    {
        return;
    }
    if engine::validate_result(&value).is_err() {
        return;
    }
    observation.snapshot(&value, &job.search);
    *previous = Some(value);
}

fn observe(observation: &mut Observation, stdout: bool, line: &str) {
    if stdout {
        observation.activity(line);
    } else {
        observation.throughput(line);
    }
}

fn persist(
    app: &App,
    job: &Job,
    observation: &Observation,
    elapsed: f64,
    base: f64,
    finished: Option<State>,
) -> Result<()> {
    app.store.update(job.id, |saved| {
        saved.total_active_seconds = base + elapsed;
        saved.progress = observation.progress.clone();
        let attempt = saved
            .attempts
            .last_mut()
            .context("Missing running attempt")?;
        attempt.elapsed_seconds = elapsed;
        attempt.search_started_at_ms = observation.search_started;
        attempt.engine_run_at_ms = observation.engine_run;
        if let Some(state) = finished {
            saved.state = state;
            saved.progress.stage = "finished".into();
            stop_live(&mut saved.progress);
            attempt.finished_at_ms = Some(now_ms());
            attempt.timing_complete = true;
            attempt.outcome = state;
        }
        Ok(())
    })?;
    Ok(())
}

async fn drain(
    mut pipe: impl AsyncRead + Unpin,
    stdout: bool,
    tx: mpsc::Sender<(bool, String)>,
    path: std::path::PathBuf,
) -> Result<()> {
    let mut log = tokio::fs::File::create(path).await?;
    let mut retained = 0;
    let mut line = Vec::new();
    let mut oversized = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        let keep = count.min((1024 * 1024_usize).saturating_sub(retained));
        log.write_all(&buffer[..keep]).await?;
        retained += keep;
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                send_line(&tx, stdout, &line, oversized);
                line.clear();
                oversized = false;
            } else if line.len() < 65536 {
                line.push(*byte);
            } else {
                oversized = true;
            }
        }
    }
}

fn send_line(tx: &mpsc::Sender<(bool, String)>, stdout: bool, line: &[u8], oversized: bool) {
    if !oversized {
        let _ = tx.try_send((stdout, String::from_utf8_lossy(line).into_owned()));
    }
}
