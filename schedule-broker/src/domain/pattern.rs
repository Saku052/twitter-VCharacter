use anyhow::{Context, Result, bail};
use chrono::{NaiveTime, Weekday};
use serde::Deserialize;

/// 生活パターン設定（availability.toml）。
/// 「予定が無い ≠ 空いている」を表現するための、カレンダーとは独立した制約。
#[derive(Debug, Clone, Deserialize)]
pub struct AvailabilityConfig {
    pub defaults: Defaults,
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub blackouts: Blackouts,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    pub timezone: String,
    #[serde(default)]
    pub buffer_before_minutes: i64,
    #[serde(default)]
    pub buffer_after_minutes: i64,
    #[serde(default = "default_min_slot")]
    pub min_slot_minutes: i64,
    /// search時に候補を刻む単位（分）
    #[serde(default = "default_granularity")]
    pub search_granularity_minutes: i64,
}

fn default_min_slot() -> i64 {
    30
}
fn default_granularity() -> i64 {
    30
}

/// 受付可能な時間帯
#[derive(Debug, Clone, Deserialize)]
pub struct Window {
    pub days: Vec<String>,
    /// [["19:00", "23:00"], ...]
    pub ranges: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Blackouts {
    #[serde(default)]
    pub ranges: Vec<BlackoutRange>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlackoutRange {
    pub days: Vec<String>,
    pub range: Vec<String>,
    #[serde(default)]
    pub label: String,
}

/// パース済みの時刻レンジ。日跨ぎ（23:00-02:00）を表現できる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRange {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl LocalRange {
    /// 終了が開始以前なら日跨ぎとみなす
    pub fn wraps_midnight(&self) -> bool {
        self.end <= self.start
    }

    /// その日のうち、この時刻が含まれるか
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn contains_time(&self, t: NaiveTime) -> bool {
        if self.wraps_midnight() {
            t >= self.start || t < self.end
        } else {
            t >= self.start && t < self.end
        }
    }
}

/// 検証済みの生活パターン。曜日→レンジ に展開済み
#[derive(Debug, Clone)]
pub struct LifePattern {
    pub timezone: chrono_tz::Tz,
    pub buffer_before_minutes: i64,
    pub buffer_after_minutes: i64,
    pub min_slot_minutes: i64,
    pub search_granularity_minutes: i64,
    /// 曜日ごとの受付可能レンジ
    windows: Vec<(Weekday, LocalRange)>,
    /// 曜日ごとの恒常的な埋まり（ラベル付き）
    blackouts: Vec<(Weekday, LocalRange, String)>,
}

impl LifePattern {
    pub fn from_config(cfg: AvailabilityConfig) -> Result<Self> {
        let timezone: chrono_tz::Tz = cfg
            .defaults
            .timezone
            .parse()
            .map_err(|_| anyhow::anyhow!("不明なタイムゾーン: {}", cfg.defaults.timezone))?;

        if cfg.defaults.min_slot_minutes <= 0 {
            bail!("min_slot_minutes は正の値である必要があります");
        }
        if cfg.defaults.search_granularity_minutes <= 0 {
            bail!("search_granularity_minutes は正の値である必要があります");
        }
        if cfg.defaults.buffer_before_minutes < 0 || cfg.defaults.buffer_after_minutes < 0 {
            bail!("バッファに負の値は指定できません");
        }

        let mut windows = Vec::new();
        for w in &cfg.windows {
            let days = parse_days(&w.days)?;
            for raw in &w.ranges {
                let range = parse_range_pair(raw)?;
                for d in &days {
                    windows.push((*d, range));
                }
            }
        }

        let mut blackouts = Vec::new();
        for b in &cfg.blackouts.ranges {
            let days = parse_days(&b.days)?;
            let range = parse_range_pair(&b.range)?;
            for d in &days {
                blackouts.push((*d, range, b.label.clone()));
            }
        }

        if windows.is_empty() {
            bail!("windows が空です。受付可能な時間帯が1つも無いと全ての問い合わせが不可になります");
        }

        Ok(Self {
            timezone,
            buffer_before_minutes: cfg.defaults.buffer_before_minutes,
            buffer_after_minutes: cfg.defaults.buffer_after_minutes,
            min_slot_minutes: cfg.defaults.min_slot_minutes,
            search_granularity_minutes: cfg.defaults.search_granularity_minutes,
            windows,
            blackouts,
        })
    }

    pub fn from_toml_str(s: &str) -> Result<Self> {
        let cfg: AvailabilityConfig =
            toml::from_str(s).context("availability.toml のパースに失敗しました")?;
        Self::from_config(cfg)
    }

    /// 指定曜日の受付可能レンジ
    pub fn windows_for(&self, day: Weekday) -> impl Iterator<Item = &LocalRange> {
        self.windows
            .iter()
            .filter(move |(d, _)| *d == day)
            .map(|(_, r)| r)
    }

    /// 指定曜日のブラックアウト
    pub fn blackouts_for(&self, day: Weekday) -> impl Iterator<Item = (&LocalRange, &str)> {
        self.blackouts
            .iter()
            .filter(move |(d, _, _)| *d == day)
            .map(|(_, r, label)| (r, label.as_str()))
    }
}

fn parse_days(days: &[String]) -> Result<Vec<Weekday>> {
    const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    let mut out = Vec::new();
    for d in days {
        match d.to_ascii_lowercase().as_str() {
            "all" => out.extend_from_slice(&ALL),
            "weekday" | "weekdays" => out.extend_from_slice(&ALL[0..5]),
            "weekend" | "weekends" => out.extend_from_slice(&ALL[5..7]),
            "mon" => out.push(Weekday::Mon),
            "tue" => out.push(Weekday::Tue),
            "wed" => out.push(Weekday::Wed),
            "thu" => out.push(Weekday::Thu),
            "fri" => out.push(Weekday::Fri),
            "sat" => out.push(Weekday::Sat),
            "sun" => out.push(Weekday::Sun),
            other => bail!("不明な曜日指定: {}", other),
        }
    }
    out.sort_by_key(|w| w.num_days_from_monday());
    out.dedup();
    Ok(out)
}

fn parse_range_pair(raw: &[String]) -> Result<LocalRange> {
    if raw.len() != 2 {
        bail!("時刻レンジは [開始, 終了] の2要素で指定してください: {:?}", raw);
    }
    let start = parse_time(&raw[0])?;
    let end = parse_time(&raw[1])?;
    Ok(LocalRange { start, end })
}

pub fn parse_time(s: &str) -> Result<NaiveTime> {
    // "24:00" は日の終わりを意味する記法として許容し、日跨ぎ扱いの 00:00 に正規化する
    if s == "24:00" {
        return Ok(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    }
    NaiveTime::parse_from_str(s, "%H:%M")
        .with_context(|| format!("時刻の形式が不正です（HH:MM で指定）: {}", s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[defaults]
timezone = "Asia/Tokyo"
buffer_before_minutes = 15
buffer_after_minutes = 15
min_slot_minutes = 30

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

    #[test]
    fn parses_sample_config() {
        let p = LifePattern::from_toml_str(SAMPLE).unwrap();
        assert_eq!(p.timezone, chrono_tz::Asia::Tokyo);
        assert_eq!(p.min_slot_minutes, 30);
        // weekday が5日に展開されている
        assert_eq!(p.windows_for(Weekday::Mon).count(), 1);
        assert_eq!(p.windows_for(Weekday::Sat).count(), 1);
        // all が7日に展開されている
        assert_eq!(p.blackouts_for(Weekday::Sun).count(), 1); // 睡眠のみ
        assert_eq!(p.blackouts_for(Weekday::Mon).count(), 2); // 勤務＋睡眠
    }

    #[test]
    fn rejects_empty_windows() {
        let toml = r#"
[defaults]
timezone = "Asia/Tokyo"
"#;
        assert!(LifePattern::from_toml_str(toml).is_err());
    }

    #[test]
    fn rejects_unknown_timezone() {
        let toml = r#"
[defaults]
timezone = "Mars/Olympus"
[[windows]]
days = ["all"]
ranges = [["10:00", "11:00"]]
"#;
        assert!(LifePattern::from_toml_str(toml).is_err());
    }

    #[test]
    fn midnight_wrapping_range_contains_correctly() {
        let r = LocalRange {
            start: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
        };
        assert!(r.wraps_midnight());
        assert!(r.contains_time(NaiveTime::from_hms_opt(23, 30, 0).unwrap()));
        assert!(r.contains_time(NaiveTime::from_hms_opt(1, 0, 0).unwrap()));
        assert!(!r.contains_time(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
    }

    #[test]
    fn normal_range_excludes_end_boundary() {
        let r = LocalRange {
            start: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
        };
        assert!(r.contains_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap()));
        assert!(!r.contains_time(NaiveTime::from_hms_opt(18, 0, 0).unwrap()));
    }
}
