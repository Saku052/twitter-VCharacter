mod config;
mod ports;
mod adapters;

use config::build_app;
use crate::ports::youtube_port::youtube_port;

#[tokio::main]
async fn main() {
    let app = build_app().await.unwrap();
    let (title, description) = app.0.get_youtube_video().await.expect("取得エラー");

    // 一旦主力してみる
    println!("title: {}\ndescription: {}", title, description)

    // 一旦出力
    
}
