use chrono::{DateTime, Datelike, Duration, NaiveTime, TimeZone, Utc};

use super::pattern::LifePattern;
use super::slot::{Availability, BusyReason, TimeSlot};

/// 空き判定エンジン。外部APIに一切依存しない純粋ロジック。
///
/// 判定順（要件定義 §3.3）:
///   1. blackouts に重なる      -> Blackout
///   2. windows に収まらない    -> OutsideWindow
///   3. カレンダーが busy       -> BusyCalendar
///   4. 未同期の予約と重なる    -> BusyPending
///   5. バッファ適用後に短い    -> TooShort
pub struct AvailabilityEngine {
    pattern: LifePattern,
}

impl AvailabilityEngine {
    pub fn new(pattern: LifePattern) -> Self {
        Self { pattern }
    }

    pub fn pattern(&self) -> &LifePattern {
        &self.pattern
    }

    /// 単一スロットの可否を判定する。
    /// `busy` はカレンダー由来、`pending` は本APIが登録しカレンダー未反映のもの。
    pub fn check(&self, slot: TimeSlot, busy: &[TimeSlot], pending: &[TimeSlot]) -> Availability {
        if slot.duration_minutes() < self.pattern.min_slot_minutes {
            return Availability::busy(
                BusyReason::TooShort,
                format!(
                    "要求された長さ {}分 が最小スロット {}分 を下回ります",
                    slot.duration_minutes(),
                    self.pattern.min_slot_minutes
                ),
            );
        }

        if let Some(label) = self.hits_blackout(&slot) {
            return Availability::busy(BusyReason::Blackout, label);
        }

        if !self.within_window(&slot) {
            return Availability::busy(
                BusyReason::OutsideWindow,
                "受付可能な時間帯の外です".to_string(),
            );
        }

        // バッファは「予定の周囲を占有する」ものなので、既存予定側を膨らませて判定する
        let before = self.pattern.buffer_before_minutes;
        let after = self.pattern.buffer_after_minutes;

        if let Some(hit) = busy.iter().find(|b| b.with_buffer(before, after).overlaps(&slot)) {
            return Availability::busy(
                BusyReason::BusyCalendar,
                format!(
                    "カレンダーに予定があります（{} - {}）",
                    self.to_local_string(hit.start),
                    self.to_local_string(hit.end)
                ),
            );
        }

        if let Some(hit) = pending.iter().find(|p| p.with_buffer(before, after).overlaps(&slot)) {
            return Availability::busy(
                BusyReason::BusyPending,
                format!(
                    "登録済みでカレンダー未反映の予約があります（{} - {}）",
                    self.to_local_string(hit.start),
                    self.to_local_string(hit.end)
                ),
            );
        }

        Availability::Free
    }

    /// 範囲内から条件を満たす空きスロットを探す。
    ///
    /// `earliest_start` / `latest_end` はローカル時刻での「1日の中の」制約。
    /// 「22時前に終わるもの」のような表現を受けるために使う。
    pub fn search(
        &self,
        range: TimeSlot,
        duration_minutes: i64,
        earliest_start: Option<NaiveTime>,
        latest_end: Option<NaiveTime>,
        busy: &[TimeSlot],
        pending: &[TimeSlot],
        limit: usize,
    ) -> Vec<TimeSlot> {
        let mut out = Vec::new();
        if limit == 0 || duration_minutes <= 0 {
            return out;
        }

        let step = Duration::minutes(self.pattern.search_granularity_minutes);
        // 探索開始点を粒度の境界に丸める（19:07 開始のような不自然な候補を避ける）
        let mut cursor = self.ceil_to_granularity(range.start);

        while cursor + Duration::minutes(duration_minutes) <= range.end {
            let Some(candidate) = TimeSlot::from_duration(cursor, duration_minutes) else {
                break;
            };

            if self.satisfies_time_of_day(&candidate, earliest_start, latest_end)
                && self.check(candidate, busy, pending).is_free()
            {
                out.push(candidate);
                if out.len() >= limit {
                    break;
                }
                // 重なる候補を返さないよう、採用したスロットの終端まで進める
                cursor = candidate.end;
                continue;
            }

            cursor += step;
        }

        out
    }

    /// ローカル時刻での時間帯制約を満たすか
    fn satisfies_time_of_day(
        &self,
        slot: &TimeSlot,
        earliest_start: Option<NaiveTime>,
        latest_end: Option<NaiveTime>,
    ) -> bool {
        let local_start = slot.start.with_timezone(&self.pattern.timezone);
        let local_end = slot.end.with_timezone(&self.pattern.timezone);

        if let Some(e) = earliest_start
            && local_start.time() < e
        {
            return false;
        }

        if let Some(l) = latest_end {
            // 日を跨ぐスロットは「その日のうちに終わる」制約を満たせない
            if local_end.date_naive() != local_start.date_naive() {
                return false;
            }
            if local_end.time() > l {
                return false;
            }
        }

        true
    }

    /// blackout に少しでも重なればラベルを返す
    fn hits_blackout(&self, slot: &TimeSlot) -> Option<String> {
        for (day, local_slot) in self.local_days_spanned(slot) {
            for (range, label) in self.pattern.blackouts_for(day) {
                for materialized in self.materialize(day_anchor(&local_slot), *range) {
                    if materialized.overlaps(slot) {
                        return Some(if label.is_empty() {
                            "恒常的に埋まっている時間帯です".to_string()
                        } else {
                            label.to_string()
                        });
                    }
                }
            }
        }
        None
    }

    /// windows のいずれかに完全に収まるか。
    /// 複数レンジに跨る場合は収まっていないとみなす（連続した空きが必要なため）
    fn within_window(&self, slot: &TimeSlot) -> bool {
        for (day, local_slot) in self.local_days_spanned(slot) {
            for range in self.pattern.windows_for(day) {
                for materialized in self.materialize(day_anchor(&local_slot), *range) {
                    if materialized.contains(slot) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// スロットが跨る各ローカル日付を列挙する。
    /// 日跨ぎのレンジを拾うため、前日から開始する
    fn local_days_spanned(&self, slot: &TimeSlot) -> Vec<(chrono::Weekday, DateTime<chrono_tz::Tz>)> {
        let tz = self.pattern.timezone;
        let start_local = slot.start.with_timezone(&tz);
        let end_local = slot.end.with_timezone(&tz);

        let mut out = Vec::new();
        let mut d = start_local.date_naive() - chrono::Days::new(1);
        let last = end_local.date_naive();

        while d <= last {
            // その日の 00:00 をアンカーにする。DST等で存在しない場合は次の有効時刻へ倒す
            if let Some(anchor) = tz
                .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .earliest()
            {
                out.push((d.weekday(), anchor));
            }
            d = d + chrono::Days::new(1);
        }
        out
    }

    /// ローカル日付上のレンジを、実時刻の区間に変換する。
    /// 日跨ぎレンジは1つの連続区間として返す
    fn materialize(
        &self,
        anchor: DateTime<chrono_tz::Tz>,
        range: super::pattern::LocalRange,
    ) -> Vec<TimeSlot> {
        let tz = self.pattern.timezone;
        let date = anchor.date_naive();

        let start_local = match tz.from_local_datetime(&date.and_time(range.start)).earliest() {
            Some(v) => v,
            None => return Vec::new(),
        };

        let end_date = if range.wraps_midnight() {
            date + chrono::Days::new(1)
        } else {
            date
        };
        let end_local = match tz.from_local_datetime(&end_date.and_time(range.end)).earliest() {
            Some(v) => v,
            None => return Vec::new(),
        };

        TimeSlot::new(start_local.with_timezone(&Utc), end_local.with_timezone(&Utc))
            .into_iter()
            .collect()
    }

    fn ceil_to_granularity(&self, t: DateTime<Utc>) -> DateTime<Utc> {
        let g = self.pattern.search_granularity_minutes;
        let tz = self.pattern.timezone;
        let local = t.with_timezone(&tz);
        let minutes = local.time().hour() as i64 * 60 + local.time().minute() as i64;
        let rem = minutes % g;
        if rem == 0 && local.time().second() == 0 {
            t
        } else {
            t + Duration::minutes(g - rem) - Duration::seconds(local.time().second() as i64)
        }
    }

    fn to_local_string(&self, t: DateTime<Utc>) -> String {
        t.with_timezone(&self.pattern.timezone)
            .format("%m/%d %H:%M")
            .to_string()
    }
}

fn day_anchor(d: &DateTime<chrono_tz::Tz>) -> DateTime<chrono_tz::Tz> {
    *d
}

use chrono::Timelike;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pattern::LifePattern;

    const PATTERN: &str = r#"
[defaults]
timezone = "Asia/Tokyo"
buffer_before_minutes = 15
buffer_after_minutes = 15
min_slot_minutes = 30
search_granularity_minutes = 30

[[windows]]
days = ["weekday"]
ranges = [["19:00", "23:00"]]

[[windows]]
days = ["sat", "sun"]
ranges = [["10:00", "23:00"]]

[blackouts]
ranges = [
  { days = ["weekday"], range = ["09:00", "18:30"], label = "勤務" },
  { days = ["all"], range = ["00:00", "07:00"], label = "睡眠" },
]
"#;

    fn engine() -> AvailabilityEngine {
        AvailabilityEngine::new(LifePattern::from_toml_str(PATTERN).unwrap())
    }

    /// JST で TimeSlot を作る
    fn jst(y: i32, m: u32, d: u32, h: u32, min: u32, dur: i64) -> TimeSlot {
        let local = chrono_tz::Asia::Tokyo
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap();
        TimeSlot::from_duration(local.with_timezone(&Utc), dur).unwrap()
    }

    #[test]
    fn weekday_daytime_is_blackout() {
        // 2026-08-26 は水曜。11:30 は勤務時間帯
        let e = engine();
        let got = e.check(jst(2026, 8, 26, 11, 30, 60), &[], &[]);
        assert_eq!(
            got,
            Availability::busy(BusyReason::Blackout, "勤務".to_string())
        );
    }

    #[test]
    fn weekday_evening_is_free() {
        let e = engine();
        assert!(e.check(jst(2026, 8, 26, 19, 30, 60), &[], &[]).is_free());
    }

    #[test]
    fn outside_window_is_rejected() {
        // 水曜 23:30 は window(19-23) の外。blackout(睡眠 0-7) には掛からない
        let e = engine();
        let got = e.check(jst(2026, 8, 26, 23, 30, 30), &[], &[]);
        assert!(matches!(
            got,
            Availability::Busy { reason: BusyReason::OutsideWindow, .. }
        ));
    }

    #[test]
    fn calendar_busy_blocks_slot() {
        let e = engine();
        let busy = vec![jst(2026, 8, 26, 19, 0, 60)];
        let got = e.check(jst(2026, 8, 26, 19, 30, 60), &busy, &[]);
        assert!(matches!(
            got,
            Availability::Busy { reason: BusyReason::BusyCalendar, .. }
        ));
    }

    #[test]
    fn buffer_blocks_adjacent_slot() {
        // 19:00-20:00 の予定があるとき、20:00-21:00 はバッファ15分により不可
        let e = engine();
        let busy = vec![jst(2026, 8, 26, 19, 0, 60)];
        let got = e.check(jst(2026, 8, 26, 20, 0, 60), &busy, &[]);
        assert!(
            matches!(got, Availability::Busy { reason: BusyReason::BusyCalendar, .. }),
            "バッファが効いていない: {:?}",
            got
        );
        // 20:15 以降なら空く
        assert!(e.check(jst(2026, 8, 26, 20, 15, 60), &busy, &[]).is_free());
    }

    #[test]
    fn pending_reservation_blocks_slot() {
        let e = engine();
        let pending = vec![jst(2026, 8, 26, 20, 0, 60)];
        let got = e.check(jst(2026, 8, 26, 20, 30, 30), &[], &pending);
        assert!(matches!(
            got,
            Availability::Busy { reason: BusyReason::BusyPending, .. }
        ));
    }

    #[test]
    fn too_short_is_rejected() {
        let e = engine();
        let got = e.check(jst(2026, 8, 26, 20, 0, 15), &[], &[]);
        assert!(matches!(
            got,
            Availability::Busy { reason: BusyReason::TooShort, .. }
        ));
    }

    #[test]
    fn weekend_daytime_is_free() {
        // 2026-08-29 は土曜。11:30 は window(10-23) 内
        let e = engine();
        assert!(e.check(jst(2026, 8, 29, 11, 30, 60), &[], &[]).is_free());
    }

    #[test]
    fn search_finds_evening_slots_only() {
        let e = engine();
        // 水曜0時から木曜0時まで探す
        let range = jst(2026, 8, 26, 0, 0, 24 * 60);
        let slots = e.search(range, 60, None, None, &[], &[], 5);

        assert!(!slots.is_empty());
        for s in &slots {
            let local = s.start.with_timezone(&chrono_tz::Asia::Tokyo);
            assert!(
                local.hour() >= 19,
                "平日昼のスロットが混入: {}",
                local
            );
        }
    }

    #[test]
    fn search_respects_latest_end_time() {
        let e = engine();
        let range = jst(2026, 8, 26, 0, 0, 24 * 60);
        let latest = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let slots = e.search(range, 60, None, Some(latest), &[], &[], 10);

        assert!(!slots.is_empty());
        for s in &slots {
            let local_end = s.end.with_timezone(&chrono_tz::Asia::Tokyo);
            assert!(
                local_end.time() <= latest,
                "22時を超えるスロットが混入: {}",
                local_end
            );
        }
    }

    #[test]
    fn search_returns_non_overlapping_slots() {
        let e = engine();
        let range = jst(2026, 8, 29, 0, 0, 24 * 60); // 土曜
        let slots = e.search(range, 60, None, None, &[], &[], 5);
        assert!(slots.len() >= 2);
        for pair in slots.windows(2) {
            assert!(
                !pair[0].overlaps(&pair[1]),
                "重なるスロットを返している: {:?} と {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn search_skips_busy_periods() {
        let e = engine();
        let range = jst(2026, 8, 26, 0, 0, 24 * 60);
        let busy = vec![jst(2026, 8, 26, 19, 0, 120)]; // 19-21時が埋まり
        let slots = e.search(range, 60, None, None, &busy, &[], 5);

        for s in &slots {
            assert!(
                !s.overlaps(&busy[0]),
                "埋まっている時間を返している: {:?}",
                s
            );
        }
        // 最初の候補は21:15以降（バッファ込み）のはず
        let first = slots.first().expect("候補が1件も無い");
        let local = first.start.with_timezone(&chrono_tz::Asia::Tokyo);
        assert!(local.hour() >= 21, "バッファを無視している: {}", local);
    }

    #[test]
    fn search_with_zero_limit_returns_empty() {
        let e = engine();
        let range = jst(2026, 8, 29, 0, 0, 24 * 60);
        assert!(e.search(range, 60, None, None, &[], &[], 0).is_empty());
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::domain::pattern::LifePattern;

    const REAL: &str = r#"
[defaults]
timezone = "Asia/Tokyo"
buffer_before_minutes = 15
buffer_after_minutes = 15
min_slot_minutes = 30
search_granularity_minutes = 30

[[windows]]
days = ["weekday"]
ranges = [["18:00", "23:00"]]

[[windows]]
days = ["sat", "sun"]
ranges = [["10:00", "23:00"]]

[blackouts]
ranges = [
  { days = ["weekday"], range = ["09:00", "17:30"], label = "勤務" },
  { days = ["all"], range = ["00:00", "07:00"], label = "睡眠" },
]
"#;

    fn jst(y: i32, m: u32, d: u32, h: u32, min: u32, dur: i64) -> TimeSlot {
        let local = chrono_tz::Asia::Tokyo.with_ymd_and_hms(y, m, d, h, min, 0).unwrap();
        TimeSlot::from_duration(local.with_timezone(&Utc), dur).unwrap()
    }

    /// 実データで発生: 18:00が塞がっているのに代替候補の先頭が18:00になる
    #[test]
    fn alternatives_must_not_include_a_busy_slot() {
        let e = AvailabilityEngine::new(LifePattern::from_toml_str(REAL).unwrap());
        // 歯医者 16:45-18:00
        let busy = vec![jst(2026, 8, 14, 16, 45, 75)];

        let requested = jst(2026, 8, 14, 18, 0, 60);
        assert!(
            !e.check(requested, &busy, &[]).is_free(),
            "18:00 はバッファにより塞がっているべき"
        );

        // check が塞がりと判定したなら、search も 18:00 を返してはいけない
        let range = TimeSlot::new(requested.start, requested.start + Duration::days(3)).unwrap();
        let alts = e.search(range, 60, None, None, &busy, &[], 3);

        for a in &alts {
            assert!(
                e.check(*a, &busy, &[]).is_free(),
                "checkが不可と判定するスロットを候補に返している: {:?}",
                a.start.with_timezone(&chrono_tz::Asia::Tokyo)
            );
        }
    }
}

#[cfg(test)]
mod constraint_tests {
    use super::*;
    use crate::domain::pattern::LifePattern;

    /// 本番と同じ設定で、ユーザーが明言した制約を守れることを確認する:
    ///   「22時以降は何も入れたくない」「7時半まで寝ている」
    fn engine() -> AvailabilityEngine {
        let src = std::fs::read_to_string("availability.toml").unwrap();
        AvailabilityEngine::new(LifePattern::from_toml_str(&src).unwrap())
    }

    fn jst(y: i32, m: u32, d: u32, h: u32, min: u32, dur: i64) -> TimeSlot {
        let local = chrono_tz::Asia::Tokyo.with_ymd_and_hms(y, m, d, h, min, 0).unwrap();
        TimeSlot::from_duration(local.with_timezone(&Utc), dur).unwrap()
    }

    #[test]
    fn nothing_starts_at_or_after_22() {
        let e = engine();
        // 2026-09-03 は木曜
        for (h, m) in [(22, 0), (22, 30), (23, 0), (23, 30)] {
            let got = e.check(jst(2026, 9, 3, h, m, 30), &[], &[]);
            assert!(!got.is_free(), "{}:{:02} が空きになっている: {:?}", h, m, got);
        }
    }

    #[test]
    fn slot_must_finish_by_22() {
        let e = engine();
        // 21:30開始の60分は22:30に終わる -> 不可
        assert!(!e.check(jst(2026, 9, 3, 21, 30, 60), &[], &[]).is_free());
        // 21:00開始の60分は22:00ちょうどに終わる -> 可
        assert!(e.check(jst(2026, 9, 3, 21, 0, 60), &[], &[]).is_free());
    }

    #[test]
    fn sleeping_until_730() {
        let e = engine();
        for (h, m) in [(0, 0), (3, 0), (6, 0), (7, 0)] {
            let got = e.check(jst(2026, 9, 5, h, m, 30), &[], &[]);
            assert!(!got.is_free(), "{}:{:02} が空きになっている", h, m);
        }
        // 土曜の朝も同様に寝ている
        assert!(!e.check(jst(2026, 9, 5, 7, 0, 30), &[], &[]).is_free());
    }

    #[test]
    fn weekend_search_never_crosses_22() {
        let e = engine();
        // 土曜1日分を探索
        let range = jst(2026, 9, 5, 0, 0, 24 * 60);
        let slots = e.search(range, 60, None, None, &[], &[], 20);
        assert!(!slots.is_empty());
        for s in &slots {
            let start = s.start.with_timezone(&chrono_tz::Asia::Tokyo);
            let end = s.end.with_timezone(&chrono_tz::Asia::Tokyo);
            assert!(start.hour() >= 10, "10時前の候補: {}", start);
            assert!(
                end.hour() < 22 || (end.hour() == 22 && end.minute() == 0),
                "22時を超える候補: {}", end
            );
        }
    }
}
