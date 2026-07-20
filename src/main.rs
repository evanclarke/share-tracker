mod app;
// API-level acceptance tests reproducing the worked examples in the ATO docs.
#[cfg(test)]
mod ato_examples;
// Tests pinning documentation-only requirements (Known-limitations entries).
#[cfg(test)]
mod doc_checks;
mod domain;
mod entities;
mod infra;
mod reports;
// Shared builder-style test fixtures used by every entity/report test module.
#[cfg(test)]
mod test_support;
mod web;

use clap::Parser;
use infra::args::Args;
use infra::{config, db, logging, scheduler};

#[tokio::main]
async fn main() {
    logging::init();

    let args = Args::parse();
    // CLI flags > config file (/usr/local/etc/share-tracker.toml or --config) >
    // built-in defaults. A bad config file aborts startup: running against the
    // wrong database is worse than not starting.
    let file = config::load(args.config.as_deref()).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let settings = config::Settings::resolve(args, file);

    let pool = db::init(&settings.db)
        .await
        .expect("failed to open database");

    // Recurring maintenance jobs are scheduled from a cron file (see `schedule.cron`),
    // not hard-coded durations. The built-in default is overridable with --schedule.
    let schedule = match &settings.schedule {
        Some(path) => std::fs::read_to_string(path).expect("failed to read schedule file"),
        None => include_str!("../schedule.cron").to_string(),
    };
    // The live price source, constructed once and shared by the scheduled
    // price-import job and the on-demand live valuation in the router.
    let fetcher: entities::closing_price::SharedFetcher =
        std::sync::Arc::new(entities::closing_price::YahooFetcher::default());
    let registry = scheduler::registry(
        pool.clone(),
        settings.db.clone(),
        settings.backup_dir.clone(),
        settings.backup_command.clone(),
        fetcher.clone(),
    );
    scheduler::spawn(registry.clone(), pool.clone(), &schedule).expect("invalid schedule");

    let app = app::router(pool.clone(), registry, fetcher);
    let ip: std::net::IpAddr = settings.host.parse().expect("invalid host address");
    let addr = std::net::SocketAddr::new(ip, settings.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    tracing::info!(
        "share-tracker v{} started, db: {}, listening on: http://{}",
        env!("CARGO_PKG_VERSION"),
        settings.db,
        addr
    );

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
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await.expect("failed to listen for ctrl+c");
}
