use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

use crate::adapters::google_calendar::GoogleCalendarClient;
use crate::domain::{AvailabilityEngine, LifePattern};
use crate::ports::calendar_reader::CalendarReader;

pub struct AppConfig {
    pub api_key: String,
    pub port: u16,
}

/// adapterを生成し、port型で返す（DI）。
/// main は具体型を知らずに CalendarReader だけを通じて操作する。
pub async fn build_app() -> Result<(impl CalendarReader + Send + Sync + 'static, AvailabilityEngine, AppConfig)> {
    dotenvy::dotenv().ok();

    let pattern_path = env::var("AVAILABILITY_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("availability.toml"));

    let pattern_src = std::fs::read_to_string(&pattern_path).with_context(|| {
        format!(
            "生活パターン設定を読み込めませんでした: {}",
            pattern_path.display()
        )
    })?;
    let pattern = LifePattern::from_toml_str(&pattern_src)?;

    let calendar = GoogleCalendarClient::new(
        env::var("GOOGLE_CLIENT_ID").context("GOOGLE_CLIENT_ID が設定されていません")?,
        env::var("GOOGLE_CLIENT_SECRET").context("GOOGLE_CLIENT_SECRET が設定されていません")?,
        env::var("GOOGLE_REFRESH_TOKEN").context("GOOGLE_REFRESH_TOKEN が設定されていません")?,
        env::var("GOOGLE_CALENDAR_IDS")
            .map(|s| {
                s.split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_else(|_| vec!["primary".to_string()]),
    );

    let config = AppConfig {
        api_key: env::var("SCHEDULE_BROKER_API_KEY")
            .context("SCHEDULE_BROKER_API_KEY が設定されていません")?,
        port: env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080),
    };

    Ok((calendar, AvailabilityEngine::new(pattern), config))
}
