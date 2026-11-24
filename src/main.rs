use anyhow::Result;
use clap::Parser;
use wifi_recovery_api::{app::App, config::Config, http, process, worker};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--licenses")
    {
        print!(
            "{}\n{}",
            include_str!("../LICENSE"),
            include_str!("../THIRD_PARTY_NOTICES.md")
        );
        return Ok(());
    }
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == "__engine-guard") {
        std::process::exit(process::guard(args.collect()).await?);
    }
    let app = App::open(Config::parse()).await?;
    let listener = tokio::net::TcpListener::bind(app.config.listen).await?;
    println!("Listening on {}", listener.local_addr()?);
    let sampler = tokio::spawn(app.clone().sample_resources());
    let mut worker = tokio::spawn(worker::run(app.clone()));
    let cancel = app.shutdown.clone();
    let mut server = tokio::spawn(
        axum::serve(listener, http::router(app.clone()))
            .with_graceful_shutdown(cancel.cancelled_owned())
            .into_future(),
    );
    tokio::select! {
        result = &mut worker => { app.shutdown.cancel(); finish_server(&mut server).await?; result??; }
        result = &mut server => { app.shutdown.cancel(); worker.await??; result??; }
        _ = shutdown_signal() => { app.shutdown.cancel(); worker.await??; finish_server(&mut server).await?; }
    }
    sampler.await?;
    Ok(())
}

async fn finish_server(server: &mut tokio::task::JoinHandle<std::io::Result<()>>) -> Result<()> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut *server).await {
        Ok(result) => result??,
        Err(_) => server.abort(),
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = signal.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
