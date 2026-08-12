use anyhow::Result;
use async_trait::async_trait;

use crate::domain::TimeSlot;

/// 本APIが作成した予約の記録。
///
/// TickTick -> Google Calendar の同期には遅延があるため、
/// 登録直後の予約は FreeBusy にまだ現れない。
/// 「自分で入れた予定を自分で見落とす」のを防ぐためにここへ残す。
#[async_trait]
pub trait ReservationStore {
    async fn insert(&self, r: NewReservation) -> Result<String>;

    /// 指定範囲に重なる有効な予約を返す
    async fn find_overlapping(&self, range: TimeSlot) -> Result<Vec<Reservation>>;

    async fn get(&self, id: &str) -> Result<Option<Reservation>>;

    async fn cancel(&self, id: &str) -> Result<bool>;
}

#[derive(Debug, Clone)]
pub struct NewReservation {
    pub task_id: String,
    pub project_id: String,
    pub title: String,
    pub slot: TimeSlot,
    pub created_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Reservation {
    /// 取得系で参照する。空き判定のみを使う経路では読まれない
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub task_id: String,
    pub project_id: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub title: String,
    pub slot: TimeSlot,
}
