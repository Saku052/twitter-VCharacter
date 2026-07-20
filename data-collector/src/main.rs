mod config;
mod ports;
mod adapters;
mod domain;

use config::build_app;
use crate::ports::{ai_generator::AiGenerator, youtube_port::YoutubePort, memo_writer::MemoWriter, qiita_port::QiitaPort, agent_port::AgentPort};

const MODEL: &str = "gpt-4o-mini";
const SYSTEM: &str = "<role>さく担当の編集者。ネタ元を読んで、さくが話したくなりそうなポイントだけ抜き出す</role>
<task>渡された情報（[YouTube動画]または[Qiita記事]）を読み、ネタにできそうな『気づき』『学び』『感想の種』を日本語の短いメモとして1個出力する</task>
<rules>
- ハッシュタグや絵文字は付けない
- 事実をそのまま要約するのではなく『これ面白いな』『これ自分でも試したい』のような感想・気づきの形に変換する
- ゲーム開発・エンジニアリングどちらの専門用語も無理に避けず、ただし社会人1年目が背伸びしすぎない温度感で
- コードや型名、ゲームエンジン特有の用語など込み入った専門用語は、メモの時点で噛み砕く（ツイート生成側では元情報を参照できないため）
- 50文字以内
</rules>";


#[tokio::main]
async fn main() {
    let app = build_app().await.unwrap();

    let mut success_count = 0;
    let mut failure_count = 0;

    // YouTube処理
    let videos = match app.0.fetch_recent_videos().await {
        Ok(videos) => videos,
        Err(e) => {
            eprintln!("YouTube動画一覧の取得に失敗: {:?}", e);
            failure_count += 1;
            Vec::new()
        }
    };
    let youtube_total = videos.len();

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

        let input = format!("[YouTube動画] タイトル: {}\n{}", video.title, video.description);
        let ai = match app.1.generate(&input, MODEL, SYSTEM).await {
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

    // Qiita処理
    let articles = match app.3.fetch_trending_articles().await {
        Ok(articles) => articles,
        Err(e) => {
            eprintln!("Qiita記事一覧の取得に失敗: {:?}", e);
            failure_count += 1;
            Vec::new()
        }
    };
    let qiita_total = articles.len();

    for article in articles {
        match app.2.is_qiita_processed(&article.article_id).await {
            Ok(true) => {
                println!("スキップ: すでに処理済み article_id={}", article.article_id);
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("処理済み判定に失敗 article_id={}: {:?}", article.article_id, e);
                failure_count += 1;
                continue;
            }
        }

        let input = format!("[Qiita記事] タイトル: {}\n{}", article.title, article.body_excerpt);
        let ai = match app.1.generate(&input, MODEL, SYSTEM).await {
            Ok(ai) => ai,
            Err(e) => {
                eprintln!("AI生成に失敗 article_id={}: {:?}", article.article_id, e);
                failure_count += 1;
                continue;
            }
        };

        let memo = format!("メモ: {}", ai);
        println!("{}", memo);

        match app.2.insert_qiita_memo(&memo, &article.article_id).await {
            Ok(()) => success_count += 1,
            Err(e) => {
                eprintln!("memo_mqへの書き込みに失敗 article_id={}: {:?}", article.article_id, e);
                failure_count += 1;
            }
        }
    }

    // Agent SDK処理
    let agent_total = match app.4.investigate().await {
        Ok(memos) => {
            let agent_total = memos.len();
            for memo in memos {
                match app.2.insert_agent_memo(&memo).await {
                    Ok(()) => {
                        println!("Agent由来メモを保存しました");
                        success_count += 1;
                    }
                    Err(e) => {
                        eprintln!("Agent由来メモのmemo_mqへの書き込みに失敗: {:?}", e);
                        failure_count += 1;
                    }
                }
            }
            agent_total
        }
        Err(e) => {
            eprintln!("Agent調査に失敗: {:?}", e);
            failure_count += 1;
            1 // 調査自体の失敗も1件の試行として数える
        }
    };
    let total = youtube_total + qiita_total + agent_total;
    println!("バッチ終了: {}件中{}件成功", total, success_count);

    if failure_count > 0 && success_count == 0 {
        eprintln!("処理を試みた{}件が全件失敗のためエラー終了します", failure_count);
        std::process::exit(1);
    }
}
