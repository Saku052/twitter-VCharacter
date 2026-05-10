mod domain;
mod ports;
mod adapters;
mod config;

use config::build_app;
use domain::post::prepare_post;
use ports::ai_generator::AiGenerator;
use ports::text_publisher::TextPublisher;

#[tokio::main]
async fn main() {

    // Clientの組み立てはconfig.rsに任せる
    let (generator, publisher) = build_app().expect("初期化失敗");

    // 文章を生成
    let content = generator.generate().expect("文章生成に失敗しました");

    // 文章を準備
    let post = prepare_post(content);

    // 投稿
    match publisher.post_text(&post).await {
        Ok(_) => println!("完了！"),
        Err(e) => eprintln!("エラー: {}", e),
    }
}

 