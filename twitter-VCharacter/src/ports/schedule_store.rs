use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::schedule::PlannedSlot;

/// 投稿予定の保管。抽選（planner）と実行（tick）を DB 越しに疎結合にする。
///
/// Railway の cron は固定時刻でしか起動できず、抽選した時刻に自分を起動できない。
/// そのため予定を先に確定させ、短間隔の cron が期限の来たものだけを拾う。
#[async_trait]
pub trait ScheduleStore {
    /// 週次の抽選結果を保存する。既に予定がある時刻は重複登録しない
    async fn save_plan(&self, slots: &[PlannedSlot]) -> Result<usize>;

    /// 予定時刻を過ぎた未実行の予定を1件取り出す。
    /// 取り出した時点で running にして、次の tick が同じ予定を拾わないようにする
    async fn claim_due_slot(&self, now: DateTime<Utc>) -> Result<Option<DueSlot>>;

    /// 実行結果を記録する
    async fn complete_slot(&self, id: i32, tweet_id: &str) -> Result<()>;

    /// 実行に失敗した予定を戻す。在庫切れなどで投稿できなかった場合に使う
    async fn release_slot(&self, id: i32, status: &str) -> Result<()>;

    /// 指定日以降の予定件数。planner が二重に週次抽選しないための確認に使う
    async fn count_pending_after(&self, from: DateTime<Utc>) -> Result<i64>;
}

/// 実行対象として取り出した予定
#[derive(Debug, Clone)]
pub struct DueSlot {
    pub id: i32,
    pub planned_at: DateTime<Utc>,
    pub block: String,
    pub planned_count: i32,
    pub seq_in_day: i32,
    pub schedule_ver: String,
}
