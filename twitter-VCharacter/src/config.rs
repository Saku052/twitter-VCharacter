use anyhow::{Context, Result};
use crate::adapters::twitter::TwitterClient;
use crate::adapters::openai::OpenAiClient;
use crate::adapters::postgres::PostgresClient;
use crate::ports::ai_generator::AiGenerator;
use crate::ports::image_generator::ImageGenerator;
use crate::ports::media_uploader::MediaUploader;
use crate::ports::memo_queue::MemoQueue;
use crate::ports::schedule_store::ScheduleStore;
use crate::ports::text_publisher::TextPublisher;

pub async fn build_app() -> Result<(
    impl AiGenerator + ImageGenerator,
    impl TextPublisher + MediaUploader,
    impl MemoQueue,
)> {
    dotenvy::dotenv().ok();

    let generator = OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY が設定されていません")?,
    );

    let publisher = TwitterClient::new(
        std::env::var("TWITTER_API_KEY").context("TWITTER_API_KEY が設定されていません")?,
        std::env::var("TWITTER_API_SECRET_KEY").context("TWITTER_API_SECRET_KEY が設定されていません")?,
        std::env::var("TWITTER_ACCESS_TOKEN").context("TWITTER_ACCESS_TOKEN が設定されていません")?,
        std::env::var("TWITTER_ACCESS_TOKEN_SECRET").context("TWITTER_ACCESS_TOKEN_SECRET が設定されていません")?
    );

    let memo_repo = PostgresClient::new(
        &std::env::var("DATABASE_URL").context("DATABASE_URL が設定されていません")?
    ).await?;

    Ok((generator, publisher, memo_repo))
}

/// DB だけを使うバイナリ（planner / tick の予定操作）向けの軽量な組み立て。
/// build_app は OpenAI と X の資格情報を要求するため、予定を触るだけの処理では重すぎる
pub async fn build_schedule_store() -> Result<impl ScheduleStore> {
    dotenvy::dotenv().ok();
    PostgresClient::new(
        &std::env::var("DATABASE_URL").context("DATABASE_URL が設定されていません")?
    ).await
}
