//! 投稿スケジュールの抽選。外部依存を持たない純粋なロジック。
//!
//! 固定3枠（08:15 / 17:55 / 21:45）をやめ、時刻と回数をランダム化する。
//! 狙いは投稿内容と時刻の交絡を断つこと。従来は FIFO でメモが枠に割り当たるため
//! 「21:45 の反応率が低い」が時刻の効果か内容の偏りか分離できなかった
//! （率比 0.69 → 内容調整後 0.80）。時刻をメモと無関係に先に決めればこれが構造的に消える。

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Asia::Tokyo;
use chrono_tz::Tz;

/// スケジュールの版。分析で期間を切り分けるために投稿へ記録する
pub const SCHEDULE_VERSION: &str = "v2-random";

/// 1日の投稿回数の候補。等確率で引く（平均3.0）。
/// 2〜5（平均3.5）では供給 2件/日 に対して在庫が約12日で尽きるため 2〜4 に抑えている
pub const DAILY_COUNTS: [usize; 3] = [2, 3, 4];

/// 同一日の投稿の最低間隔。閲覧の半減期が約80分であることから逆算
pub const MIN_GAP_MINUTES: i64 = 120;

/// 投稿を割り当てる時間帯。0〜7時は除外（深夜はネット利用率が数%で、見られないまま沈む）。
/// 勤務時間は除外しない（プロダクトオーナーの判断）。
/// ブロックは「割付を均等にする単位」であって分析の単位ではない。
/// 分析では時刻を連続量として扱うため、粒度は粗くてよい。
pub const BLOCKS: [(&str, u32, u32); 6] = [
    ("morning", 7, 10),   // 07:00-10:00
    ("midday", 10, 13),   // 10:00-13:00
    ("afternoon", 13, 16), // 13:00-16:00
    ("evening", 16, 19),  // 16:00-19:00
    ("night", 19, 22),    // 19:00-22:00
    ("latenight", 22, 24), // 22:00-24:00
];

/// 抽選された1件の予定
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedSlot {
    pub planned_at: DateTime<Tz>,
    pub block: String,
    /// その日の計画投稿数。在庫不足で実際に減っても、分析では計画値を使う
    /// （intention-to-treat）ため記録する
    pub planned_count: usize,
    /// その日の何本目か。日内の投稿どうしの干渉を測るのに使う
    pub seq_in_day: usize,
}

/// 抽選に使う乱数。テストから決定的に差し替えられるようにトレイトにする
pub trait Rng {
    /// 0 以上 max 未満の整数を返す
    fn next_below(&mut self, max: usize) -> usize;
}

/// 線形合同法。暗号用途ではないが、割付の無作為化には十分
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6364136223846793005).wrapping_add(1))
    }
}

impl Rng for Lcg {
    fn next_below(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % max
    }
}

/// 1週間分（start_date から7日）の投稿予定を抽選する。
///
/// 置換ブロック法で週単位に均す。日ごとに独立に引くと、週20回の週と週10回の週が
/// 生まれて供給計画が立たなくなるため、週の合計を先に決めてから日に配る。
///
/// `weekly_total` を指定すると週の合計をその値に寄せる（供給に連動させるためのパラメータ）。
/// None なら DAILY_COUNTS の平均 × 7 になる。
pub fn plan_week<R: Rng>(
    start_date: NaiveDate,
    weekly_total: Option<usize>,
    rng: &mut R,
) -> Vec<PlannedSlot> {
    let counts = daily_counts_for_week(weekly_total, rng);
    let mut block_usage = vec![0usize; BLOCKS.len()];
    let mut out = Vec::new();

    for (day_offset, &count) in counts.iter().enumerate() {
        let date = start_date + Duration::days(day_offset as i64);
        let slots = plan_day(date, count, &mut block_usage, rng);
        out.extend(slots);
    }
    out
}

/// 週の合計を7日に配る。合計を保ったまま各日を DAILY_COUNTS の範囲に収める
fn daily_counts_for_week<R: Rng>(weekly_total: Option<usize>, rng: &mut R) -> Vec<usize> {
    let min = *DAILY_COUNTS.iter().min().unwrap();
    let max = *DAILY_COUNTS.iter().max().unwrap();

    let target = weekly_total.unwrap_or_else(|| {
        let sum: usize = DAILY_COUNTS.iter().sum();
        sum * 7 / DAILY_COUNTS.len()
    });
    // 実現不可能な合計は範囲内に丸める
    let target = target.clamp(min * 7, max * 7);

    // 最小値から始めて、残りを無作為な日に1ずつ足す
    let mut counts = vec![min; 7];
    let mut remaining = target - min * 7;
    while remaining > 0 {
        let candidates: Vec<usize> = (0..7).filter(|&i| counts[i] < max).collect();
        if candidates.is_empty() {
            break;
        }
        let pick = candidates[rng.next_below(candidates.len())];
        counts[pick] += 1;
        remaining -= 1;
    }

    // 並びの偏りを消す（前半に多い、のような癖をなくす）
    for i in (1..counts.len()).rev() {
        let j = rng.next_below(i + 1);
        counts.swap(i, j);
    }
    counts
}

/// 1日分の時刻を決める。使用回数の少ないブロックを優先して選び、週を通して割付を均す
fn plan_day<R: Rng>(
    date: NaiveDate,
    count: usize,
    block_usage: &mut [usize],
    rng: &mut R,
) -> Vec<PlannedSlot> {
    let mut chosen: Vec<usize> = Vec::new();

    for _ in 0..count.min(BLOCKS.len()) {
        // まだこの日に使っていないブロックのうち、週での使用回数が最小のものを候補にする
        let min_usage = (0..BLOCKS.len())
            .filter(|i| !chosen.contains(i))
            .map(|i| block_usage[i])
            .min();
        let Some(min_usage) = min_usage else { break };

        let candidates: Vec<usize> = (0..BLOCKS.len())
            .filter(|i| !chosen.contains(i) && block_usage[*i] == min_usage)
            .collect();
        let pick = candidates[rng.next_below(candidates.len())];
        chosen.push(pick);
        block_usage[pick] += 1;
    }

    // ブロック順に並べてから時刻を引く。最低間隔を満たせない場合はそのブロックを諦める
    chosen.sort_unstable();

    let mut slots: Vec<PlannedSlot> = Vec::new();
    for &bi in &chosen {
        let (name, from_h, to_h) = BLOCKS[bi];
        if let Some(at) = draw_time(date, from_h, to_h, slots.last().map(|s| s.planned_at), rng) {
            slots.push(PlannedSlot {
                planned_at: at,
                block: name.to_string(),
                planned_count: count,
                seq_in_day: slots.len() + 1,
            });
        }
    }
    slots
}

/// ブロック内から分単位で時刻を引く。直前の予定から MIN_GAP_MINUTES 空くまで引き直す。
/// :00 や :15 のような切りの良い時刻は結果として避けられる
fn draw_time<R: Rng>(
    date: NaiveDate,
    from_h: u32,
    to_h: u32,
    prev: Option<DateTime<Tz>>,
    rng: &mut R,
) -> Option<DateTime<Tz>> {
    let span_min = ((to_h - from_h) * 60) as usize;

    for _ in 0..32 {
        let offset = rng.next_below(span_min);
        let minutes = from_h as i64 * 60 + offset as i64;
        let time = NaiveTime::from_hms_opt((minutes / 60) as u32 % 24, (minutes % 60) as u32, 0)?;
        let naive = date.and_time(time);
        // 夏時間のない Asia/Tokyo では single になる
        let at = Tokyo.from_local_datetime(&naive).single()?;

        match prev {
            Some(p) if (at - p) < Duration::minutes(MIN_GAP_MINUTES) => continue,
            _ => return Some(at),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn weekly_total_is_respected() {
        let mut rng = Lcg::new(42);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);
        assert_eq!(slots.len(), 21);
    }

    #[test]
    fn weekly_total_can_follow_supply() {
        // 供給が細いときは週合計を下げて運用する
        let mut rng = Lcg::new(7);
        let slots = plan_week(date(2026, 10, 5), Some(14), &mut rng);
        assert_eq!(slots.len(), 14);
    }

    #[test]
    fn daily_count_stays_within_allowed_range() {
        let mut rng = Lcg::new(99);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);

        let mut per_day = std::collections::BTreeMap::new();
        for s in &slots {
            *per_day.entry(s.planned_at.date_naive()).or_insert(0usize) += 1;
        }
        for (d, c) in per_day {
            assert!(DAILY_COUNTS.contains(&c), "{} の投稿数 {} が範囲外", d, c);
        }
    }

    #[test]
    fn all_times_are_inside_allowed_hours() {
        let mut rng = Lcg::new(1234);
        for seed_day in 0..10 {
            let slots = plan_week(date(2026, 10, 5) + Duration::days(seed_day * 7), Some(21), &mut rng);
            for s in slots {
                let h = s.planned_at.format("%H").to_string().parse::<u32>().unwrap();
                assert!((7..24).contains(&h), "{} は許可時間帯の外", s.planned_at);
            }
        }
    }

    #[test]
    fn minimum_gap_is_kept_within_a_day() {
        let mut rng = Lcg::new(2026);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);

        let mut by_day: std::collections::BTreeMap<_, Vec<_>> = Default::default();
        for s in &slots {
            by_day.entry(s.planned_at.date_naive()).or_default().push(s.planned_at);
        }
        for (d, mut times) in by_day {
            times.sort();
            for w in times.windows(2) {
                let gap = w[1] - w[0];
                assert!(
                    gap >= Duration::minutes(MIN_GAP_MINUTES),
                    "{} の間隔 {} 分が最低間隔を下回る",
                    d,
                    gap.num_minutes()
                );
            }
        }
    }

    #[test]
    fn seq_in_day_is_sequential() {
        let mut rng = Lcg::new(555);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);

        let mut by_day: std::collections::BTreeMap<_, Vec<usize>> = Default::default();
        for s in &slots {
            by_day.entry(s.planned_at.date_naive()).or_default().push(s.seq_in_day);
        }
        for (_, seqs) in by_day {
            let expected: Vec<usize> = (1..=seqs.len()).collect();
            assert_eq!(seqs, expected);
        }
    }

    #[test]
    fn blocks_are_used_evenly_across_the_week() {
        let mut rng = Lcg::new(31337);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);

        let mut usage = std::collections::BTreeMap::new();
        for s in &slots {
            *usage.entry(s.block.clone()).or_insert(0usize) += 1;
        }
        let max = *usage.values().max().unwrap();
        let min = *usage.values().min().unwrap();
        // 置換ブロック法なので、週を通した偏りは1以内に収まるはず
        assert!(max - min <= 1, "ブロックの使用回数が偏っている: {:?}", usage);
    }

    #[test]
    fn same_day_never_reuses_a_block() {
        let mut rng = Lcg::new(808);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);

        let mut by_day: std::collections::BTreeMap<_, Vec<String>> = Default::default();
        for s in &slots {
            by_day.entry(s.planned_at.date_naive()).or_default().push(s.block.clone());
        }
        for (d, blocks) in by_day {
            let mut uniq = blocks.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), blocks.len(), "{} で同じブロックを重複使用", d);
        }
    }

    #[test]
    fn times_are_not_always_on_the_hour() {
        // 固定時刻に見えないこと（分が散らばること）を確認する
        let mut rng = Lcg::new(4649);
        let slots = plan_week(date(2026, 10, 5), Some(21), &mut rng);
        let distinct_minutes: std::collections::BTreeSet<String> =
            slots.iter().map(|s| s.planned_at.format("%M").to_string()).collect();
        assert!(distinct_minutes.len() > 5, "分の散らばりが乏しい: {:?}", distinct_minutes);
    }

    #[test]
    fn plan_is_deterministic_for_a_given_seed() {
        let a = plan_week(date(2026, 10, 5), Some(21), &mut Lcg::new(11));
        let b = plan_week(date(2026, 10, 5), Some(21), &mut Lcg::new(11));
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_give_different_plans() {
        let a = plan_week(date(2026, 10, 5), Some(21), &mut Lcg::new(11));
        let b = plan_week(date(2026, 10, 5), Some(21), &mut Lcg::new(12));
        assert_ne!(a, b);
    }

    #[test]
    fn impossible_weekly_total_is_clamped() {
        // 週70回は1日あたり上限を超えるので、達成可能な範囲に丸める
        let mut rng = Lcg::new(3);
        let slots = plan_week(date(2026, 10, 5), Some(70), &mut rng);
        assert_eq!(slots.len(), DAILY_COUNTS.iter().max().unwrap() * 7);
    }
}
