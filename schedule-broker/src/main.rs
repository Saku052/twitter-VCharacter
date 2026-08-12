mod adapters;
mod config;
mod domain;
mod http;
mod ports;

use std::sync::Arc;

use config::build_app;
use http::{AppState, router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "schedule_broker=info,tower_http=warn".into()),
        )
        .init();

    let (calendar, engine, app_config) = build_app().await?;

    let state = Arc::new(AppState {
        calendar: Box::new(calendar),
        engine,
        api_key: app_config.api_key,
    });

    let addr = format!("0.0.0.0:{}", app_config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("schedule-broker を起動しました: {}", addr);

    axum::serve(listener, router(state)).await?;
    Ok(())
}
