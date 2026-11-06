use std::time::Duration;

use asksms::{AppState, Config, app};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    tracing::info!("listening on {}", listener.local_addr()?);

    axum::serve(listener, app(AppState::new(config, http)))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await.ok();
}
