use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::sync::Mutex;

use crate::domain::TimeSlot;
use crate::ports::calendar_reader::CalendarReader;

const FREEBUSY_URL: &str = "https://www.googleapis.com/calendar/v3/freeBusy";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google Calendar FreeBusy APIで埋まり区間を取得する。
///
/// refresh_token から access_token を都度発行する方式（Phase Aでは永続化しない）。
/// access_token はプロセス内でのみキャッシュし、期限切れ前に再発行する。
pub struct GoogleCalendarClient {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    /// 空きを判定する対象カレンダーID。"primary" が既定
    calendar_ids: Vec<String>,
    cached_token: Mutex<Option<CachedToken>>,
    token_url: String,
    freebusy_url: String,
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: DateTime<Utc>,
}

impl GoogleCalendarClient {
    pub fn new(
        client_id: String,
        client_secret: String,
        refresh_token: String,
        calendar_ids: Vec<String>,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            refresh_token,
            calendar_ids: if calendar_ids.is_empty() {
                vec!["primary".to_string()]
            } else {
                calendar_ids
            },
            cached_token: Mutex::new(None),
            token_url: TOKEN_URL.to_string(),
            freebusy_url: FREEBUSY_URL.to_string(),
        }
    }

    #[cfg(test)]
    fn with_urls(mut self, token_url: String, freebusy_url: String) -> Self {
        self.token_url = token_url;
        self.freebusy_url = freebusy_url;
        self
    }

    async fn access_token(&self) -> Result<String> {
        let mut guard = self.cached_token.lock().await;

        // 期限の60秒前を切っていたら再発行する
        if let Some(cached) = guard.as_ref()
            && cached.expires_at > Utc::now() + Duration::seconds(60)
        {
            return Ok(cached.access_token.clone());
        }

        let client = reqwest::Client::new();
        let response = client
            .post(&self.token_url)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("Googleトークンエンドポイントへの接続に失敗しました")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("アクセストークンの取得に失敗しました (status={}): {}", status, body);
        }

        let body: TokenResponse = response
            .json()
            .await
            .context("トークンレスポンスの解釈に失敗しました")?;

        let token = CachedToken {
            access_token: body.access_token.clone(),
            expires_at: Utc::now() + Duration::seconds(body.expires_in),
        };
        *guard = Some(token);

        Ok(body.access_token)
    }
}

#[async_trait]
impl CalendarReader for GoogleCalendarClient {
    async fn fetch_busy(&self, range: TimeSlot) -> Result<Vec<TimeSlot>> {
        let token = self.access_token().await?;

        let items: Vec<HashMap<&str, &str>> = self
            .calendar_ids
            .iter()
            .map(|id| HashMap::from([("id", id.as_str())]))
            .collect();

        let request = serde_json::json!({
            "timeMin": range.start.to_rfc3339(),
            "timeMax": range.end.to_rfc3339(),
            "items": items,
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&self.freebusy_url)
            .bearer_auth(&token)
            .json(&request)
            .send()
            .await
            .context("FreeBusy APIへの接続に失敗しました")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("FreeBusyの取得に失敗しました (status={}): {}", status, body);
        }

        let body: FreeBusyResponse = response
            .json()
            .await
            .context("FreeBusyレスポンスの解釈に失敗しました")?;

        let mut slots = Vec::new();
        for (calendar_id, calendar) in body.calendars {
            // 個別カレンダーのエラーは全体を落とさず警告に留める。
            // 1つのカレンダーの権限不足で全ての判定が止まるのを避けるため
            if let Some(errors) = calendar.errors
                && !errors.is_empty()
            {
                tracing::warn!(
                    calendar_id = %calendar_id,
                    "カレンダーの読み取りでエラーが返りました: {:?}",
                    errors
                );
                continue;
            }

            for period in calendar.busy {
                match TimeSlot::new(period.start, period.end) {
                    Some(slot) => slots.push(slot),
                    None => tracing::warn!(
                        calendar_id = %calendar_id,
                        "長さが0以下の busy 区間を無視しました: {} - {}",
                        period.start,
                        period.end
                    ),
                }
            }
        }

        slots.sort_by_key(|s| s.start);
        Ok(slots)
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: i64,
}

fn default_expires_in() -> i64 {
    3600
}

#[derive(Deserialize)]
struct FreeBusyResponse {
    #[serde(default)]
    calendars: HashMap<String, CalendarBusy>,
}

#[derive(Deserialize)]
struct CalendarBusy {
    #[serde(default)]
    busy: Vec<BusyPeriod>,
    #[serde(default)]
    errors: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct BusyPeriod {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn range() -> TimeSlot {
        TimeSlot::new(
            Utc.with_ymd_and_hms(2026, 8, 26, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 8, 27, 0, 0, 0).unwrap(),
        )
        .unwrap()
    }

    async fn setup(freebusy_body: serde_json::Value) -> (MockServer, GoogleCalendarClient) {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "test-token",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/freebusy"))
            .respond_with(ResponseTemplate::new(200).set_body_json(freebusy_body))
            .mount(&server)
            .await;

        let client = GoogleCalendarClient::new(
            "id".into(),
            "secret".into(),
            "refresh".into(),
            vec!["primary".into()],
        )
        .with_urls(
            format!("{}/token", server.uri()),
            format!("{}/freebusy", server.uri()),
        );

        (server, client)
    }

    #[tokio::test]
    async fn parses_busy_periods() {
        let (_server, client) = setup(serde_json::json!({
            "calendars": {
                "primary": {
                    "busy": [
                        { "start": "2026-08-26T10:00:00Z", "end": "2026-08-26T11:00:00Z" }
                    ]
                }
            }
        }))
        .await;

        let slots = client.fetch_busy(range()).await.unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].duration_minutes(), 60);
    }

    #[tokio::test]
    async fn skips_calendars_with_errors() {
        let (_server, client) = setup(serde_json::json!({
            "calendars": {
                "primary": {
                    "busy": [
                        { "start": "2026-08-26T10:00:00Z", "end": "2026-08-26T11:00:00Z" }
                    ]
                },
                "broken@example.com": {
                    "busy": [],
                    "errors": [{ "domain": "global", "reason": "notFound" }]
                }
            }
        }))
        .await;

        let slots = client.fetch_busy(range()).await.unwrap();
        assert_eq!(slots.len(), 1, "エラーのあるカレンダーで全体が落ちてはいけない");
    }

    #[tokio::test]
    async fn empty_calendar_yields_no_slots() {
        let (_server, client) = setup(serde_json::json!({
            "calendars": { "primary": { "busy": [] } }
        }))
        .await;

        assert!(client.fetch_busy(range()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn results_are_sorted_by_start() {
        let (_server, client) = setup(serde_json::json!({
            "calendars": {
                "a": { "busy": [{ "start": "2026-08-26T15:00:00Z", "end": "2026-08-26T16:00:00Z" }] },
                "b": { "busy": [{ "start": "2026-08-26T09:00:00Z", "end": "2026-08-26T10:00:00Z" }] }
            }
        }))
        .await;

        let slots = client.fetch_busy(range()).await.unwrap();
        assert_eq!(slots.len(), 2);
        assert!(slots[0].start < slots[1].start);
    }
}
