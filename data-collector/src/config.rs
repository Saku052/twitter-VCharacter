use crate::ports::ai_generator;
use crate::ports::youtube_port::youtube_port;
use crate::adapters::youtube::youtube_client;
use crate::ports::ai_generator::AiGenerator;
use crate::adapters::openai::OpenAiClient;
use anyhow::Result;
use std::env;


pub async fn build_app() -> Result<(impl youtube_port, impl AiGenerator)> {
    //一旦envファイルを読み込む
    dotenvy::dotenv().ok();
    let youtube_client = youtube_client::new(
        env::var("YOUTUBE_API_KEY").expect("don't know the api key")
    );
    let openai_client = OpenAiClient::new(
        env::var("OPENAI_API_KEY").expect("don't know the api key")
    );

    Ok((youtube_client, openai_client))
}
