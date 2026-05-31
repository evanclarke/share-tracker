mod amma;
mod amit_adjustment;
mod args;
mod ato_fx_rate;
mod portfolio;
mod db;
mod decimal;
mod exchange;
mod income;
mod listing;
mod logging;
mod parcel_allocation;
mod realised_gains;
mod sell;
mod tax_summary;
mod trade;
mod unrealised_gains;

use args::Args;
use clap::Parser;

#[tokio::main]
async fn main() {
    logging::init();

    let args = Args::parse();

    let pool = db::init(&args.db).await.expect("failed to open database");
    db::spawn_daily_backup(pool.clone(), args.db.clone());

    let app = exchange::router()
        .merge(listing::router())
        .merge(ato_fx_rate::router())
        .merge(trade::router())
        .merge(income::router())
        .merge(amma::router())
        .merge(parcel_allocation::router())
        .merge(sell::router())
        .merge(amit_adjustment::router())
        .merge(portfolio::router())
        .merge(unrealised_gains::router())
        .merge(realised_gains::router())
        .merge(tax_summary::router())
        .with_state(pool.clone());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("failed to bind");
    tracing::info!("share-tracker started, db: {}, port: {}", args.db, args.port);

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
