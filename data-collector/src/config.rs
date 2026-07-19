use crate::ports::youtube_port::YoutubePort;
use crate::adapters::youtube::YoutubeClient;
use crate::ports::ai_generator::AiGenerator;
use crate::adapters::openai::OpenAiClient;
use crate::ports::memo_writer::MemoWriter;
use crate::adapters::postgres::PostgresClient;
use crate::ports::qiita_port::QiitaPort;
use crate::adapters::qiita::QiitaClient;
use crate::ports::agent_port::AgentPort;
use crate::adapters::agent::AgentClient;
use anyhow::Result;
use std::env;


pub async fn build_app() -> Result<(impl YoutubePort, impl AiGenerator, impl MemoWriter, impl QiitaPort, impl AgentPort)> {
    //一旦envファイルを読み込む
    dotenvy::dotenv().ok();
    let youtube_client = YoutubeClient::new(
        env::var("YOUTUBE_API_KEY").expect("don't know the api key")
    );
    let openai_client = OpenAiClient::new(
        env::var("OPENAI_API_KEY").expect("don't know the api key")
    );
    let memo_repo = PostgresClient::new(
        &std::env::var("DATABASE_URL").expect("DATABASE_URL が設定されていません")
    ).await?;
    let qiita_client = QiitaClient::new();
    let agent_client = AgentClient::new(
        env::var("AGENT_SDK_URL").expect("AGENT_SDK_URL が設定されていません"),
        env::var("AGENT_SDK_API_KEY").expect("AGENT_SDK_API_KEY が設定されていません"),
    );

    Ok((youtube_client, openai_client, memo_repo, qiita_client, agent_client))
}
