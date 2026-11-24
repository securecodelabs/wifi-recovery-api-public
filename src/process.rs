use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
};

pub fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("Another process owns this data directory or engine")?;
    Ok(file)
}

pub async fn wait_guard(path: &Path) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        match lock(path) {
            Ok(file) => {
                drop(file);
                return Ok(());
            }
            Err(error) if tokio::time::Instant::now() >= deadline => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

async fn bounded(mut stream: impl AsyncRead + Unpin, capacity: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        ensure!(
            output.len() + read <= capacity,
            "Engine validation output exceeded limit"
        );
        output.extend_from_slice(&buffer[..read]);
    }
}

pub async fn probe(engine: &Path, args: &[OsString], timeout: Duration) -> Result<String> {
    let mut child = Command::new(engine)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("Missing probe stdout")?;
    let stderr = child.stderr.take().context("Missing probe stderr")?;
    let operation = async {
        let (stdout, stderr, status) = tokio::try_join!(
            bounded(stdout, 32 * 1024 * 1024),
            bounded(stderr, 64 * 1024),
            async { Ok::<_, anyhow::Error>(child.wait().await?) }
        )?;
        ensure!(
            status.success(),
            "Engine rejected request: {}",
            String::from_utf8_lossy(&stderr)
        );
        Ok(String::from_utf8(stdout)?)
    };
    tokio::time::timeout(timeout, operation)
        .await
        .context("Engine validation timed out")?
}

pub async fn stop(child: &mut Child) -> Result<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
    }
    #[cfg(not(unix))]
    child.start_kill()?;
    if let Ok(status) = tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
        return Ok(status?);
    }
    child.kill().await?;
    Ok(child.wait().await?)
}

/// Same executable acts as a parent-pipe guardian. No helper binary is distributed.
pub async fn guard(args: Vec<OsString>) -> Result<i32> {
    ensure!(args.len() >= 3, "Invalid internal guardian arguments");
    let _ownership = lock(Path::new(&args[0]))?;
    #[cfg(unix)]
    {
        let expected: i32 = args[1].to_string_lossy().parse()?;
        ensure!(
            nix::unistd::getppid().as_raw() == expected,
            "API parent exited before engine startup"
        );
    }
    let mut child = Command::new(&args[2])
        .args(&args[3..])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = tokio::io::stdin();
    let mut byte = [0_u8];
    let status = tokio::select! {
        status = child.wait() => status?,
        _ = input.read(&mut byte) => stop(&mut child).await?,
    };
    Ok(status.code().unwrap_or(1))
}
