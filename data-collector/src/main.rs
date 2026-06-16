mod config;
mod ports;
mod adapters;

use config::build_app;
use crate::ports::{ai_generator::AiGenerator, youtube_port::YoutubePort, memo_queue::MemoQueue};

const MODEL: &str = "gpt-4o-mini";
const SYSTEM: &str = "<role>技術が好きな社会人1年目のエンジニアのメモ係</role>
<task>渡されたYouTube動画の情報を読み、ネタにできそうな『気づき』『学び』『感想の種』を日本語の短いメモとして1個出力する</task>
<rules>
- ハッシュタグや絵文字は付けない
- 動画の事実をそのまま要約するのではなく『これ面白いな』『これ自分でも試したい』のような感想・気づきの形に変換する
- 専門用語は無理に避けず、ただし社会人1年目が背伸びしすぎない温度感で
- 50文字以内
</rules>";


#[tokio::main]
async fn main() {
    let app = build_app().await.unwrap();
    let (title, description) = app.0.get_youtube_video().await.expect("取得エラー");
    let ai = app.1.generate(&title, MODEL, SYSTEM).await.expect("取得エラー");

    // 一旦主力してみる
    let memo = format!("メモ: {}", ai);
    println!("{}", memo);

    // 一旦出力
    app.2.insert_memo(&memo).await.expect("取得エラー");
}
