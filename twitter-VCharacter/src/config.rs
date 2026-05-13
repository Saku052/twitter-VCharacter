use anyhow::{Context, Result};
use crate::adapters::twitter::TwitterClient;
use crate::adapters::openai::OpenAiClient;
use crate::ports::ai_generator::AiGenerator;
use crate::ports::text_publisher::TextPublisher;

pub fn build_app() -> Result<(impl AiGenerator, impl TextPublisher)> {
    dotenvy::dotenv().ok();

    let generator = OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY が設定されていません")?,
        "gpt-4o-mini".to_string(),
        "あなたはさくというゲームクリエイターのVTuberです。140字以内でツイートを生成してください。".to_string(),
    );

    let publisher = TwitterClient::new(
        std::env::var("TWITTER_API_KEY").context("TWITTER_API_KEY が設定されていません")?,
        std::env::var("TWITTER_API_SECRET_KEY").context("TWITTER_API_SECRET_KEY が設定されていません")?,
        std::env::var("TWITTER_ACCESS_TOKEN").context("TWITTER_ACCESS_TOKEN が設定されていません")?,
        std::env::var("TWITTER_ACCESS_TOKEN_SECRET").context("TWITTER_ACCESS_TOKEN_SECRET が設定されていません")?
    );

    Ok((generator, publisher))
}
