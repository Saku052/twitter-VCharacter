use crate::ports::youtube_port::YoutubePort;
use crate::adapters::youtube::YoutubeClient;
use crate::ports::ai_generator::AiGenerator;
use crate::adapters::openai::OpenAiClient;
use crate::ports::memo_writer::MemoWriter;
use crate::adapters::postgres::PostgresClient;
use crate::ports::qiita_port::QiitaPort;
use crate::adapters::qiita::QiitaClient;
use anyhow::Result;
use std::env;


pub async fn build_app() -> Result<(impl YoutubePort, impl AiGenerator, impl MemoWriter, impl QiitaPort)> {
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

    Ok((youtube_client, openai_client, memo_repo, qiita_client))
}
