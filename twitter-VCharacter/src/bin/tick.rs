//! 期限の来た投稿予定を実行する。Railway の cron で5分おきに起動する想定。
//!
//! 大半の起動では予定がなく、何もせずに終わる。
//!
//! Railway の cron は前の実行が終わるまで次をスキップする（キューにも入らない）ため、
//! 生成 API がハングすると以降の投稿が静かに全部止まる。これを避けるため全体に
//! タイムアウトを掛け、5分の起動間隔より確実に短く打ち切る。

use anyhow::{Context, Result};
use chrono::Utc;
use std::time::Duration;
use tokio::time::timeout;

use vcharacter::config::build_schedule_store;
use vcharacter::ports::schedule_store::ScheduleStore;
use vcharacter::publish::publish_once;

/// 1回の起動の上限。cron 間隔（5分）より短くして、次の起動を止めない
const RUN_TIMEOUT: Duration = Duration::from_secs(230);

#[tokio::main]
async fn main() {
    match timeout(RUN_TIMEOUT, run()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            eprintln!("エラー: {:#}", e);
            std::process::exit(1);
        }
        Err(_) => {
            // ここで落としておかないと、次以降の cron が延々とスキップされる
            eprintln!("エラー: {}秒で完了しなかったため打ち切りました", RUN_TIMEOUT.as_secs());
            std::process::exit(1);
        }
    }
}

async fn run() -> Result<()> {
    let store = build_schedule_store().await?;

    let Some(slot) = store
        .claim_due_slot(Utc::now())
        .await
        .context("予定の取得に失敗しました")?
    else {
        // 予定がないのが通常。ログを汚さないよう静かに終わる
        return Ok(());
    };

    println!(
        "予定を実行します id={} planned_at={} block={} {}本目/{}",
        slot.id,
        slot.planned_at.format("%Y-%m-%d %H:%M"),
        slot.block,
        slot.seq_in_day,
        slot.planned_count
    );

    match publish_once().await {
        Ok(tweet_id) => {
            store
                .complete_slot(slot.id, &tweet_id)
                .await
                .context("予定の完了記録に失敗しました")?;
            println!("完了 id={} tweet_id={}", slot.id, tweet_id);
            Ok(())
        }
        Err(e) => {
            // 在庫切れや品質ガードの全滅。予定は skipped にして次の予定へ進む。
            // pending に戻すと期限切れまで毎 tick 再試行してしまうため戻さない
            store
                .release_slot(slot.id, "skipped")
                .await
                .context("予定の解放に失敗しました")?;
            Err(e).context(format!("投稿に失敗したため予定を skipped にしました id={}", slot.id))
        }
    }
}
