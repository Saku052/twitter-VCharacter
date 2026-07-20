mod domain;
mod ports;
mod adapters;
mod config;

use config::build_app;
use domain::post::{parse_tags, prepare_post};
use ports::ai_generator::AiGenerator;
use ports::image_generator::ImageGenerator;
use ports::media_uploader::MediaUploader;
use ports::memo_queue::MemoQueue;
use ports::text_publisher::TextPublisher;

const GPT_MODEL: &str = "ft:gpt-4.1-2025-04-14:personal:tweetsource1:DfS5fKl8";
const BODY_SYS_PRPT: &str = "<role>技術が好きな社会人1年目のエンジニア</role>
<task>渡されたメモを元に、本人視点のツイート本文を生成する</task>
<rules>
- 140字以内
- ハッシュタグは含めない（別途生成するため）
- 砕けた口語（「〜なんだよな」「〜じゃん」「〜かもしれない」など）と断定調を内容に応じて使い分け
- 絵文字は0〜2個、内容に応じて自然に配置
- 自慢や説教にならず、気づきや失敗を等身大で書く
</rules>";

const TAG_SYS_PRPT: &str = "<role>技術が好きな社会人1年目のエンジニア</role>
<task>渡されたメモを元に、ツイートに付けるハッシュタグを考える</task>
<rules>
- 内容に関連するタグを1〜2個
- 「#」は付けず、カンマ区切りで出力する（例: Rust,個人開発）
- 説明文や前置きは付けず、タグの文字列のみを出力する
</rules>";

const IMAGE_PROMPT_TEMPLATE: &str = "以下のメモの雰囲気を表す、シンプルで温かみのあるイラスト画像を1枚生成してください。
文字は入れないでください。技術系VTuberのツイート添付を想定した、
柔らかい色合いのフラットイラスト風。

メモ: {memo}";

#[tokio::main]
async fn main() {
    // Clientの組み立てはconfig.rsに任せる
    let (generator, publisher, memo_repo) = build_app().await.expect("初期化失敗");

    // メモを取得
    let memo = memo_repo.fetch_latest_memo().await.expect("メモの取得に失敗しました");

    // 文章を生成
    // TODO: 本当はmemoの部分はDBに制約をつけておいた方が良い
    // TODO: これ普通にmemoも方も直接とる（unwrap_or_defaultじゃない方法）とか何のか？
    let memoid: i32 = memo.id;
    let memo_text = memo.memo.unwrap_or_default();

    let body = generator.generate(&memo_text, GPT_MODEL, BODY_SYS_PRPT).await.expect("本文生成に失敗しました");
    let tags_raw = generator.generate(&memo_text, GPT_MODEL, TAG_SYS_PRPT).await.expect("タグ生成に失敗しました");
    let tags = parse_tags(&tags_raw);

    // 文章を準備
    let post = prepare_post(body, tags);

    // 画像生成（確率判定。失敗時は画像なしで投稿続行）
    let probability: f64 = std::env::var("IMAGE_POST_PROBABILITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.3);

    let media_ids = if rand::random::<f64>() < probability {
        let image_prompt = IMAGE_PROMPT_TEMPLATE.replace("{memo}", &memo_text);
        match generator.generate_image(&image_prompt).await {
            Ok(image_bytes) => match publisher.upload_media(&image_bytes).await {
                Ok(media_id) => Some(vec![media_id]),
                Err(e) => {
                    eprintln!("画像アップロード失敗、テキストのみで続行: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("画像生成失敗、テキストのみで続行: {}", e);
                None
            }
        }
    } else {
        None
    };

    // 投稿
    match publisher.post_text(&post, media_ids).await {
        Ok(_) => {
            memo_repo.mark_used_memo(memoid).await.expect("メモの更新に失敗");
            println!("完了！")
        },
        Err(e) => eprintln!("エラー: {}", e),
    }
}
