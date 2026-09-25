mod domain;
mod ports;
mod adapters;
mod config;

use config::build_app;
use domain::post::{parse_tags, prepare_post, validate_body};
// 画像生成は一旦廃止（Phase5でペルソナ転換に伴い停止。復活の可能性があるためコメントアウトで残置）
// use domain::post::{parse_image_post_probability, should_attach_image};
use ports::ai_generator::AiGenerator;
// use ports::image_generator::ImageGenerator;
// use ports::media_uploader::MediaUploader;
use ports::memo_queue::MemoQueue;
use ports::text_publisher::TextPublisher;

// 旧モデル（fine-tuned、技術者ペルソナで学習済み）。切り戻し用に保持: ft:gpt-4.1-2025-04-14:personal:tweetsource1:DfS5fKl8
// Phase5でfine-tuning運用をやめ、素のgpt-5.5 + プロンプトのみでペルソナを表現する方式に切り替え
const GPT_MODEL: &str = "gpt-5.5";
const BODY_SYS_PRPT: &str = "<role>個人でゲームを作っているVtuber。普段は社会人1年目のエンジニアとして働いていて、その経験を活かして自分のゲームを作っている</role>
<task>渡されたメモを元に、本人視点のツイート本文を生成する</task>
<rules>
- 140字以内
- ハッシュタグは含めない（別途生成するため）
- 砕けた口語（「〜なんだよな」「〜じゃん」「〜かもしれない」など）と断定調を内容に応じて使い分け
- 絵文字は0〜2個、内容に応じて自然に配置
- 自慢や説教にならず、気づきや失敗を等身大で書く
- 渡されたメモがツイートの素材として成立していない（作業報告、指示や依頼、見出しだけ、意味をなさない断片など）場合は、本文を書かずに「SKIP」とだけ出力する
</rules>";
// ↑ 最後の規則は品質ガード（G3）。「SKIP」は domain::post::SKIP_SENTINEL と一致させる。
// 2026-09-25 導入（プロンプト v1.1）。反響分析ではこの日を境に期間を分ける

/// 出力ガードで弾かれたときに、同じ実行の中で試すメモの上限。枠を空振りしないため
const MAX_ATTEMPTS: usize = 3;

const TAG_SYS_PRPT: &str = "<role>個人でゲームを作っているVtuber。普段は社会人1年目のエンジニアとして働いていて、その経験を活かして自分のゲームを作っている</role>
<task>渡されたメモを元に、ツイートに付けるハッシュタグを考える</task>
<rules>
- 内容に関連するタグを1〜2個
- 「#」は付けず、カンマ区切りで出力する（例: Rust,個人開発）
- 説明文や前置きは付けず、タグの文字列のみを出力する
</rules>";

// 画像生成機能は一旦廃止（Phase5でペルソナ転換に伴い停止）。IMAGE_PROMPT_TEMPLATEも含め復活の可能性があるため関連コードは残置。
// const IMAGE_PROMPT_TEMPLATE: &str = "以下のメモの雰囲気を表す、シンプルで温かみのあるイラスト画像を1枚生成してください。
// 文字は入れないでください。ゲーム開発系Vtuberのツイート添付を想定した、
// 柔らかい色合いのフラットイラスト風。
//
// メモ: {memo}";

/// 失敗を Railway 上で FAILED として見えるようにする（終了コード0のままだと SUCCESS 表示になり気づけない）
fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("エラー: {}", msg);
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    // Clientの組み立てはconfig.rsに任せる
    let (generator, publisher, memo_repo) = build_app().await.unwrap_or_else(|e| fail(format!("初期化失敗: {}", e)));

    for attempt in 1..=MAX_ATTEMPTS {
        // メモを取得（未使用かつ未スキップの最古1件）
        let memo = memo_repo.fetch_latest_memo().await.unwrap_or_else(|e| fail(format!("メモの取得に失敗しました: {}", e)));

        // TODO: 本当はmemoの部分はDBに制約をつけておいた方が良い
        // TODO: これ普通にmemoも方も直接とる（unwrap_or_defaultじゃない方法）とか何のか？
        let memoid: i32 = memo.id;
        let memo_text = memo.memo.unwrap_or_default();

        let body = generator.generate(&memo_text, GPT_MODEL, BODY_SYS_PRPT).await
            .unwrap_or_else(|e| fail(format!("本文生成に失敗しました: {}", e)));

        // 出力ガード（G4）。弾いた本文は投稿せず、メモをスキップ扱いにして次のメモを試す
        if let Err(reason) = validate_body(&body) {
            let code = reason.code();
            eprintln!("出力ガードで棄却 attempt={} memo_id={} reason={}: {:?}", attempt, memoid, code, body);
            memo_repo.mark_skipped_memo(memoid, &code).await
                .unwrap_or_else(|e| fail(format!("スキップの記録に失敗しました memo_id={}: {}", memoid, e)));
            continue;
        }

        let tags_raw = generator.generate(&memo_text, GPT_MODEL, TAG_SYS_PRPT).await
            .unwrap_or_else(|e| fail(format!("タグ生成に失敗しました: {}", e)));
        let tags = parse_tags(&tags_raw);

        // 文章を準備
        // 記録用に保持してから結合する（prepare_post は所有権を取るため）
        let body_for_record = body.clone();
        let tags_for_record = tags.join(" ");
        let post = prepare_post(body, tags);

        // 画像生成は一旦廃止（Phase5でペルソナ転換に伴い停止。復活の可能性があるためコメントアウトで残置）
        // let probability = parse_image_post_probability(std::env::var("IMAGE_POST_PROBABILITY").ok());
        //
        // let media_ids = if should_attach_image(rand::random::<f64>(), probability) {
        //     let image_prompt = IMAGE_PROMPT_TEMPLATE.replace("{memo}", &memo_text);
        //     match generator.generate_image(&image_prompt).await {
        //         Ok(image_bytes) => match publisher.upload_media(&image_bytes).await {
        //             Ok(media_id) => Some(vec![media_id]),
        //             Err(e) => {
        //                 eprintln!("画像アップロード失敗、テキストのみで続行: {}", e);
        //                 None
        //             }
        //         },
        //         Err(e) => {
        //             eprintln!("画像生成失敗、テキストのみで続行: {}", e);
        //             None
        //         }
        //     }
        // } else {
        //     None
        // };
        let media_ids = None;

        // 投稿。失敗したメモは未使用のまま残り、次の枠で再挑戦される
        let tweet_id = publisher.post_text(&post, media_ids).await
            .unwrap_or_else(|e| fail(format!("投稿に失敗しました memo_id={}: {}", memoid, e)));

        memo_repo.mark_used_memo(memoid).await
            .unwrap_or_else(|e| fail(format!("投稿は成功したがメモの更新に失敗 tweet_id={}: {}", tweet_id, e)));

        // 反響分析の起点。記録に失敗しても投稿自体は成功しているので、
        // ここで落とさず警告に留める
        if let Err(e) = memo_repo
            .record_posted_tweet(
                &tweet_id,
                memoid,
                &body_for_record,
                &tags_for_record,
                memo.source.as_deref(),
            )
            .await
        {
            eprintln!("投稿の記録に失敗しました (tweet_id={}): {}", tweet_id, e);
        }

        println!("完了！");
        return;
    }

    // 連続で弾かれるのはキュー側の異常（壊れたメモの流入）とみなして知らせる
    fail(format!("{}件連続で出力ガードに弾かれたため投稿を見送りました", MAX_ATTEMPTS));
}
