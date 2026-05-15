use anyhow::{Context, Result};
use crate::adapters::twitter::TwitterClient;
use crate::adapters::openai::OpenAiClient;
use crate::ports::ai_generator::AiGenerator;
use crate::ports::text_publisher::TextPublisher;

pub fn build_app() -> Result<(impl AiGenerator, impl TextPublisher)> {
    dotenvy::dotenv().ok();

    let generator = OpenAiClient::new(
        std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY が設定されていません")?,
        "ft:gpt-4.1-2025-04-14:personal:tweetsource1:DfS5fKl8".to_string(),
        "<role>技術が好きな社会人1年目のエンジニア</role>\n<task>渡されたメモを元に、本人視点のツイートを生成する</task>\n<rules>\n- 140字以内（ハッシュタグ含む）\n- 砕けた口語（「〜なんだよな」「〜じゃん」「〜かもしれない」など）と断定調を内容に応じて使い分け\n- 絵文字は0〜2個、内容に応じて自然に配置\n- 自慢や説教にならず、気づきや失敗を等身大で書く\n- 内容に関連するハッシュタグを1〜2個、本文と空行を挟んで末尾に配置\n</rules>".to_string(),
    );

    let publisher = TwitterClient::new(
        std::env::var("TWITTER_API_KEY").context("TWITTER_API_KEY が設定されていません")?,
        std::env::var("TWITTER_API_SECRET_KEY").context("TWITTER_API_SECRET_KEY が設定されていません")?,
        std::env::var("TWITTER_ACCESS_TOKEN").context("TWITTER_ACCESS_TOKEN が設定されていません")?,
        std::env::var("TWITTER_ACCESS_TOKEN_SECRET").context("TWITTER_ACCESS_TOKEN_SECRET が設定されていません")?
    );

    Ok((generator, publisher))
}
