//! 週次の投稿予定を抽選して DB に書き込む。
//!
//! Railway の cron で週1回（月曜の未明）起動する想定。
//! 投稿そのものは行わない。予定を置くだけで、実行は post-tick が拾う。
//!
//! 週合計 `WEEKLY_POST_TOTAL` は供給に連動させるパラメータ。
//! メモの供給が細いあいだは小さくし、回復したら 21 に戻す。分布の形は変えない。

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use chrono_tz::Asia::Tokyo;

use vcharacter::config::build_schedule_store;
use vcharacter::domain::schedule::{plan_week, Lcg};
use vcharacter::ports::schedule_store::ScheduleStore;

/// 供給が足りない間の既定値。{2,3,4} の平均3 × 7日 = 21 が本来の値
const DEFAULT_WEEKLY_TOTAL: usize = 21;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("予定の作成に失敗しました: {:#}", e);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let store = build_schedule_store().await?;

    let weekly_total = std::env::var("WEEKLY_POST_TOTAL")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_WEEKLY_TOTAL);

    // 翌日から1週間分を組む。当日を含めないのは、すでに過ぎた時刻の予定を作らないため
    let start = (Utc::now() + Duration::days(1)).with_timezone(&Tokyo).date_naive();

    // 予定が十分残っているなら作らない（cron の重複起動や手動再実行に耐える）
    let pending = store
        .count_pending_after(Utc::now())
        .await
        .context("既存の予定数を数えられませんでした")?;
    if pending >= weekly_total as i64 {
        println!("予定が {} 件残っているため作成をスキップしました", pending);
        return Ok(());
    }

    // 週ごとに違う並びになるよう、開始日から種を作る
    let seed = start
        .signed_duration_since(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
        .num_days() as u64;
    let mut rng = Lcg::new(seed);

    let slots = plan_week(start, Some(weekly_total), &mut rng);
    let saved = store.save_plan(&slots).await.context("予定の保存に失敗しました")?;

    println!(
        "{} から1週間分の予定を作成しました: 抽選 {} 件 / 保存 {} 件（週合計 {}）",
        start,
        slots.len(),
        saved,
        weekly_total
    );
    for s in &slots {
        println!("  {} [{}] {}本目/{}", s.planned_at.format("%m-%d %H:%M"), s.block, s.seq_in_day, s.planned_count);
    }
    Ok(())
}
