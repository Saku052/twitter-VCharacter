//! 1件の投稿を最後まで行う。main.rs と tick から共通で呼ぶ。
//! 生成・品質ガード・記録のロジックをひとつに保つため、ここに集約する。

use anyhow::{bail, Result};

use crate::config::build_app;
use crate::domain::post::{parse_tags, prepare_post, validate_body};
use crate::ports::ai_generator::AiGenerator;
use crate::ports::memo_queue::MemoQueue;
use crate::ports::text_publisher::TextPublisher;

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

/// 投稿を1件行い、成功したら tweet_id を返す
pub async fn publish_once() -> Result<String> {
    let (generator, publisher, memo_repo) = build_app().await?;

    for attempt in 1..=MAX_ATTEMPTS {
        let memo = memo_repo.fetch_latest_memo().await?;
        let memoid: i32 = memo.id;
        let memo_text = memo.memo.clone().unwrap_or_default();

        let body = generator.generate(&memo_text, GPT_MODEL, BODY_SYS_PRPT).await?;

        // 出力ガード（G4）。弾いた本文は投稿せず、メモをスキップ扱いにして次のメモを試す
        if let Err(reason) = validate_body(&body) {
            let code = reason.code();
            eprintln!("出力ガードで棄却 attempt={} memo_id={} reason={}: {:?}", attempt, memoid, code, body);
            memo_repo.mark_skipped_memo(memoid, &code).await?;
            continue;
        }

        let tags_raw = generator.generate(&memo_text, GPT_MODEL, TAG_SYS_PRPT).await?;
        let tags = parse_tags(&tags_raw);

        // 記録用に保持してから結合する（prepare_post は所有権を取るため）
        let body_for_record = body.clone();
        let tags_for_record = tags.join(" ");
        let post = prepare_post(body, tags);

        let tweet_id = publisher.post_text(&post, None).await?;
        memo_repo.mark_used_memo(memoid).await?;

        // 反響分析の起点。記録に失敗しても投稿自体は成功しているので警告に留める
        if let Err(e) = memo_repo
            .record_posted_tweet(&tweet_id, memoid, &body_for_record, &tags_for_record, memo.source.as_deref())
            .await
        {
            eprintln!("投稿の記録に失敗しました (tweet_id={}): {}", tweet_id, e);
        }

        return Ok(tweet_id);
    }

    // 連続で弾かれるのはキュー側の異常（壊れたメモの流入）とみなして知らせる
    bail!("{}件連続で出力ガードに弾かれたため投稿を見送りました", MAX_ATTEMPTS)
}
