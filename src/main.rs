mod app;
// API-level acceptance tests reproducing the worked examples in the ATO docs.
#[cfg(test)]
mod ato_examples;
mod entities;
mod infra;
mod reports;
mod web;

use clap::Parser;
use infra::args::Args;
use infra::{db, logging, scheduler};

#[tokio::main]
async fn main() {
    logging::init();

    let args = Args::parse();

    let pool = db::init(&args.db).await.expect("failed to open database");

    // Recurring maintenance jobs are scheduled from a cron file (see `schedule.cron`),
    // not hard-coded durations. The built-in default is overridable with --schedule.
    let schedule = match &args.schedule {
        Some(path) => std::fs::read_to_string(path).expect("failed to read schedule file"),
        None => include_str!("../schedule.cron").to_string(),
    };
    let registry = scheduler::registry(pool.clone(), args.db.clone());
    scheduler::spawn(registry.clone(), pool.clone(), &schedule).expect("invalid schedule");

    let app = app::router(pool.clone(), registry);
    let ip: std::net::IpAddr = args.host.parse().expect("invalid --host address");
    let addr = std::net::SocketAddr::new(ip, args.port);
    let listener = tokio::net::TcpListener::bind(addr).await.expect("failed to bind");
    tracing::info!("share-tracker started, db: {}, listening on: http://{}", args.db, addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    tracing::info!("shutting down");
    pool.close().await;
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await.expect("failed to listen for ctrl+c");
}
