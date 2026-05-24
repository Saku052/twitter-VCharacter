mod domain;
mod ports;
mod adapters;
mod config;

use config::build_app;
use domain::post::prepare_post;
use ports::ai_generator::AiGenerator;
use ports::memo_queue::MemoQueue;
use ports::text_publisher::TextPublisher;

const GPT_MODEL: &str = "ft:gpt-4.1-2025-04-14:personal:tweetsource1:DfS5fKl8";
const SYS_PRPT: &str = "<role>技術が好きな社会人1年目のエンジニア</role>\n<task>渡されたメモを元に、
本人視点のツイートを生成する</task>\n<rules>\n- 140字以内（ハッシュタグ含む）\n- 砕けた口語
（「〜なんだよな」「〜じゃん」「〜かもしれない」など）と断定調を内容に応じて使い分け\n- 絵文字は0〜2個、
内容に応じて自然に配置\n- 自慢や説教にならず、気づきや失敗を等身大で書く\n- 内容に関連するハッシュタグを
1〜2個、本文と空行を挟んで末尾に配置\n</rules>";

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
    let content = generator.generate(&memo.memo.unwrap_or_default(), GPT_MODEL, SYS_PRPT).await.expect("文章生成に失敗しました");

    // 文章を準備
    let post = prepare_post(content);

    // 投稿
    match publisher.post_text(&post).await {
        Ok(_) => {
            memo_repo.mark_used_memo(memoid).await.expect("メモの更新に失敗");
            println!("完了！")
        },
        Err(e) => eprintln!("エラー: {}", e),
    }
}
