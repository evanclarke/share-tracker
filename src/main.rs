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
use infra::args::{Args, Command};
use infra::{config, db, logging, scheduler};

#[tokio::main]
async fn main() {
    let args = Args::parse();
    // The one-off `[auth]`-populating helpers run instead of the server, and
    // need no logging, config file or database — see `infra::args::Command`.
    if let Some(command) = &args.command {
        run_command(command);
        return;
    }

    logging::init();

    // CLI flags > config file (/usr/local/etc/share-tracker.toml or --config) >
    // built-in defaults. A bad config file aborts startup: running against the
    // wrong database is worse than not starting.
    let file = config::load(args.config.as_deref()).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let settings = config::Settings::resolve(args, file).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    if settings.auth.is_none() && settings.host != "127.0.0.1" {
        tracing::warn!(
            "no authentication configured; listening on {} without access control \
             (see the README's \"Authentication\" section)",
            settings.host
        );
    }

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

    let app = app::router(
        &settings.base_path,
        pool.clone(),
        registry,
        fetcher,
        settings.auth.clone(),
    );
    let ip: std::net::IpAddr = settings.host.parse().expect("invalid host address");
    let addr = std::net::SocketAddr::new(ip, settings.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    // The base path is part of the URL every request must use, so log it with
    // the address: a misconfigured reverse-proxy prefix is then diagnosable
    // from the startup line rather than only from 404s.
    tracing::info!(
        "share-tracker v{} started, db: {}, listening on: http://{}{}/",
        env!("CARGO_PKG_VERSION"),
        settings.db,
        addr,
        settings.base_path
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    tracing::info!("shutting down");
    pool.close().await;
}

/// Runs a `[auth]`-populating helper and exits. Neither touches the config
/// file or database — each just prints a value the operator pastes into
/// `[auth]` by hand.
fn run_command(command: &Command) {
    match command {
        Command::HashPassword => {
            use std::io::Read;
            let mut password = String::new();
            std::io::stdin()
                .read_to_string(&mut password)
                .expect("failed to read password from stdin");
            // Trailing newline from an interactive Enter or `echo` (not
            // `echo -n`) is not part of the intended password.
            let password = password.trim_end_matches(['\n', '\r']);
            match infra::auth::Auth::hash_password(password) {
                Ok(hash) => println!("{hash}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Command::GenToken => println!("{}", infra::auth::Auth::generate_token()),
    }
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
