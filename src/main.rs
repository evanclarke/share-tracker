mod args;
mod db;

use args::Args;
use clap::Parser;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let pool = db::init(&args.db).await.expect("failed to open database");

    db::spawn_daily_backup(pool.clone(), args.db.clone());

    println!("share-tracker started, db: {}", args.db);
    // TODO: start axum server

    shutdown_signal().await;
    println!("shutting down");
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
