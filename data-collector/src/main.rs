mod config;
mod ports;
mod adapters;
mod domain;

use config::build_app;
use crate::ports::{ai_generator::AiGenerator, youtube_port::YoutubePort, memo_writer::MemoWriter};

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
    let videos = app.0.fetch_recent_videos().await.expect("取得エラー");

    let total = videos.len();
    let mut success_count = 0;
    let mut failure_count = 0;

    for video in videos {
        match app.2.is_processed(&video.video_id).await {
            Ok(true) => {
                println!("スキップ: すでに処理済み video_id={}", video.video_id);
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("処理済み判定に失敗 video_id={}: {:?}", video.video_id, e);
                failure_count += 1;
                continue;
            }
        }

        let ai = match app.1.generate(&video.title, MODEL, SYSTEM).await {
            Ok(ai) => ai,
            Err(e) => {
                eprintln!("AI生成に失敗 video_id={}: {:?}", video.video_id, e);
                failure_count += 1;
                continue;
            }
        };

        let memo = format!("メモ: {}", ai);
        println!("{}", memo);

        match app.2.insert_memo(&memo, &video.video_id).await {
            Ok(()) => success_count += 1,
            Err(e) => {
                eprintln!("memo_mqへの書き込みに失敗 video_id={}: {:?}", video.video_id, e);
                failure_count += 1;
            }
        }
    }

    println!("バッチ終了: {}件中{}件成功", total, success_count);

    if failure_count > 0 && success_count == 0 {
        eprintln!("処理を試みた{}件が全件失敗のためエラー終了します", failure_count);
        std::process::exit(1);
    }
}
