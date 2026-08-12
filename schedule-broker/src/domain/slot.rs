use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// 時間の区間。start < end を常に満たす（コンストラクタで保証）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TimeSlot {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeSlot {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    pub fn from_duration(start: DateTime<Utc>, minutes: i64) -> Option<Self> {
        Self::new(start, start + Duration::minutes(minutes))
    }

    pub fn duration_minutes(&self) -> i64 {
        (self.end - self.start).num_minutes()
    }

    /// 半開区間 [start, end) として重なりを判定する。
    /// 隣接（前の予定のendと次の予定のstartが一致）は重ならないものとして扱う
    pub fn overlaps(&self, other: &TimeSlot) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn contains(&self, other: &TimeSlot) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    /// 前後にバッファを付けた区間を返す
    pub fn with_buffer(&self, before_minutes: i64, after_minutes: i64) -> TimeSlot {
        TimeSlot {
            start: self.start - Duration::minutes(before_minutes),
            end: self.end + Duration::minutes(after_minutes),
        }
    }
}

/// 空き判定の結果。空いていない場合は理由を持つ
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Availability {
    Free,
    Busy { reason: BusyReason, detail: String },
}

impl Availability {
    pub fn is_free(&self) -> bool {
        matches!(self, Availability::Free)
    }

    pub fn busy(reason: BusyReason, detail: impl Into<String>) -> Self {
        Availability::Busy { reason, detail: detail.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusyReason {
    /// カレンダーに予定が入っている
    BusyCalendar,
    /// 本APIが登録済みでカレンダーへ未同期の予約がある
    BusyPending,
    /// 恒常的に埋まっている帯（勤務・睡眠など）
    Blackout,
    /// 受付可能な時間帯の外
    OutsideWindow,
    /// バッファ適用後に最小スロット長を満たさない
    TooShort,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn new_rejects_zero_and_negative_length() {
        let t = utc(2026, 8, 26, 10, 0);
        assert!(TimeSlot::new(t, t).is_none());
        assert!(TimeSlot::new(t, t - Duration::minutes(1)).is_none());
        assert!(TimeSlot::new(t, t + Duration::minutes(1)).is_some());
    }

    #[test]
    fn adjacent_slots_do_not_overlap() {
        // 10:00-11:00 と 11:00-12:00 は重ならない（半開区間）
        let a = TimeSlot::new(utc(2026, 8, 26, 10, 0), utc(2026, 8, 26, 11, 0)).unwrap();
        let b = TimeSlot::new(utc(2026, 8, 26, 11, 0), utc(2026, 8, 26, 12, 0)).unwrap();
        assert!(!a.overlaps(&b));
        assert!(!b.overlaps(&a));
    }

    #[test]
    fn overlapping_slots_are_detected_both_ways() {
        let a = TimeSlot::new(utc(2026, 8, 26, 10, 0), utc(2026, 8, 26, 12, 0)).unwrap();
        let b = TimeSlot::new(utc(2026, 8, 26, 11, 0), utc(2026, 8, 26, 13, 0)).unwrap();
        assert!(a.overlaps(&b));
        assert!(b.overlaps(&a));
    }

    #[test]
    fn contains_is_inclusive_at_boundaries() {
        let outer = TimeSlot::new(utc(2026, 8, 26, 9, 0), utc(2026, 8, 26, 18, 0)).unwrap();
        let same = TimeSlot::new(utc(2026, 8, 26, 9, 0), utc(2026, 8, 26, 18, 0)).unwrap();
        let inner = TimeSlot::new(utc(2026, 8, 26, 10, 0), utc(2026, 8, 26, 11, 0)).unwrap();
        let crossing = TimeSlot::new(utc(2026, 8, 26, 17, 0), utc(2026, 8, 26, 19, 0)).unwrap();
        assert!(outer.contains(&same));
        assert!(outer.contains(&inner));
        assert!(!outer.contains(&crossing));
    }
}
