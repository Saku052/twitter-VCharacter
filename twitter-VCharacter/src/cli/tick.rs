//! 期限の来た投稿予定を実行する（サブコマンド `tick`）。
//!
//! Railway の cron は前の実行が終わるまで次をスキップする（キューにも入らない）ため、
//! 生成 API がハングすると以降の投稿が静かに全部止まる。呼び出し側でタイムアウトを掛ける。

use anyhow::{Context, Result};
use chrono::Utc;

use crate::config::build_schedule_store;
use crate::ports::schedule_store::ScheduleStore;
use crate::publish::publish_once;

pub async fn run() -> Result<()> {
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
