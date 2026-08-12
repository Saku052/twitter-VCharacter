use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

use crate::adapters::google_calendar::GoogleCalendarClient;
use crate::adapters::postgres::PostgresClient;
use crate::adapters::ticktick::TickTickClient;
use crate::domain::{AvailabilityEngine, LifePattern};
use crate::ports::calendar_reader::CalendarReader;
use crate::ports::reservation_store::ReservationStore;
use crate::ports::task_writer::TaskWriter;

pub struct AppConfig {
    pub api_key: String,
    pub port: u16,
}

/// 書き込み側（Phase B）。未設定でも読み取りだけで起動できるようにする
pub struct WriteSide {
    pub tasks: Box<dyn TaskWriter + Send + Sync>,
    pub reservations: Box<dyn ReservationStore + Send + Sync>,
}

type BuiltApp = (
    Box<dyn CalendarReader + Send + Sync>,
    Option<WriteSide>,
    AvailabilityEngine,
    AppConfig,
);

/// adapterを生成し、port型で返す（DI）。
/// main は具体型を知らずに port だけを通じて操作する。
pub async fn build_app() -> Result<BuiltApp> {
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

    // 書き込み側は TickTick トークンと DB が揃ったときだけ有効にする。
    // 片方だけの状態で予約を受けると、TickTickに入ったのに記録が残らない等の
    // 不整合が起きるため、両方揃うことを条件にする
    let ticktick_token = env::var("TICKTICK_ACCESS_TOKEN").ok().filter(|s| !s.is_empty());
    let database_url = env::var("DATABASE_URL").ok().filter(|s| !s.is_empty());

    let write_side = match (ticktick_token, database_url) {
        (Some(token), Some(db_url)) => {
            let store = PostgresClient::new(&db_url).await?;
            Some(WriteSide {
                tasks: Box::new(TickTickClient::new(
                    token,
                    env::var("TICKTICK_PROJECT_ID").ok(),
                )),
                reservations: Box::new(store),
            })
        }
        (token, db) => {
            tracing::warn!(
                "書き込み機能を無効にして起動します（TICKTICK_ACCESS_TOKEN={}, DATABASE_URL={}）",
                if token.is_some() { "設定済み" } else { "未設定" },
                if db.is_some() { "設定済み" } else { "未設定" },
            );
            None
        }
    };

    let config = AppConfig {
        api_key: env::var("SCHEDULE_BROKER_API_KEY")
            .context("SCHEDULE_BROKER_API_KEY が設定されていません")?,
        port: env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080),
    };

    Ok((
        Box::new(calendar),
        write_side,
        AvailabilityEngine::new(pattern),
        config,
    ))
}
