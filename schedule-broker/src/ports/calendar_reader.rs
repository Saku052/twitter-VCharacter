use anyhow::Result;
use async_trait::async_trait;

use crate::domain::TimeSlot;

/// カレンダーから「埋まっている区間」を読む。
/// Phase A ではGoogle Calendar FreeBusy APIが実装する。
#[async_trait]
pub trait CalendarReader {
    async fn fetch_busy(&self, range: TimeSlot) -> Result<Vec<TimeSlot>>;
}
