use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, FixedOffset, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::domain::pattern::parse_time;
use crate::domain::{Availability, AvailabilityEngine, TimeSlot};
use crate::ports::calendar_reader::CalendarReader;

/// searchの探索範囲上限。無制限だとFreeBusyのクォータを浪費するため
const MAX_SEARCH_DAYS: i64 = 60;
const MAX_LIMIT: usize = 50;

pub struct AppState {
    pub calendar: Box<dyn CalendarReader + Send + Sync>,
    pub engine: AvailabilityEngine,
    pub api_key: String,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/availability/check", post(check))
        .route("/v1/availability/search", post(search))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

// ---------- check ----------

#[derive(Deserialize)]
pub struct CheckRequest {
    start: DateTime<FixedOffset>,
    duration_minutes: i64,
}

#[derive(Serialize)]
pub struct CheckResponse {
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    alternatives: Vec<SlotView>,
}

async fn check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<CheckRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<CheckResponse>, ApiError> {
    authorize(&headers, &state.api_key)?;
    let Json(req) = body.map_err(|e| ApiError::bad_request("invalid_body", e.body_text()))?;

    if req.duration_minutes <= 0 {
        return Err(ApiError::bad_request(
            "invalid_duration",
            "duration_minutes は正の値で指定してください",
        ));
    }

    let start = req.start.with_timezone(&Utc);
    let slot = TimeSlot::from_duration(start, req.duration_minutes).ok_or_else(|| {
        ApiError::bad_request("invalid_range", "有効な時間区間になりません")
    })?;

    // 判定にはバッファ分だけ広い範囲の予定が要る
    let margin = Duration::minutes(
        state.engine.pattern().buffer_before_minutes + state.engine.pattern().buffer_after_minutes,
    ) + Duration::hours(1);
    let fetch_range = TimeSlot::new(slot.start - margin, slot.end + margin)
        .ok_or_else(|| ApiError::bad_request("invalid_range", "取得範囲が不正です"))?;

    // 生活パターンだけで不可と分かる場合、カレンダーを引かずに答える。
    // 上流が落ちていても「勤務中です」は返せるべきで、かつFreeBusyのクォータを節約できる
    if let Availability::Busy { reason, detail } = state.engine.check(slot, &[], &[]) {
        let alternatives = match alternatives_for(&state, slot, req.duration_minutes).await {
            Ok(alts) => alts,
            // 代替候補は付加情報。取得できなくても判定自体は返す
            Err(e) => {
                tracing::warn!("代替候補の取得に失敗しました: {:?}", e);
                Vec::new()
            }
        };
        return Ok(Json(CheckResponse {
            available: false,
            reason: Some(reason_code(reason)),
            detail: Some(detail),
            alternatives,
        }));
    }

    // ここから先はカレンダーを見ないと判断できない
    let busy = state
        .calendar
        .fetch_busy(fetch_range)
        .await
        .map_err(ApiError::upstream)?;

    let response = match state.engine.check(slot, &busy, &[]) {
        Availability::Free => CheckResponse {
            available: true,
            reason: None,
            detail: None,
            alternatives: Vec::new(),
        },
        Availability::Busy { reason, detail } => CheckResponse {
            available: false,
            reason: Some(reason_code(reason)),
            detail: Some(detail),
            alternatives: alternatives_for(&state, slot, req.duration_minutes)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("代替候補の取得に失敗しました: {:?}", e);
                    Vec::new()
                }),
        },
    };

    Ok(Json(response))
}

/// 不可だった場合の代替候補を、当日から3日先まで探す
async fn alternatives_for(
    state: &Arc<AppState>,
    slot: TimeSlot,
    duration_minutes: i64,
) -> anyhow::Result<Vec<SlotView>> {
    let range = TimeSlot::new(slot.start, slot.start + Duration::days(3))
        .ok_or_else(|| anyhow::anyhow!("代替探索範囲が不正です"))?;

    // 探索範囲の直前で終わる予定もバッファ経由で候補を塞ぐため、手前に余裕を持って取得する。
    // これを怠ると「塞がりと判定した時刻」を代替候補として返してしまう
    let margin = Duration::minutes(
        state.engine.pattern().buffer_before_minutes + state.engine.pattern().buffer_after_minutes,
    ) + Duration::hours(1);
    let fetch_range = TimeSlot::new(range.start - margin, range.end)
        .ok_or_else(|| anyhow::anyhow!("代替探索の取得範囲が不正です"))?;
    let busy = state.calendar.fetch_busy(fetch_range).await?;

    Ok(state
        .engine
        .search(range, duration_minutes, None, None, &busy, &[], 3)
        .into_iter()
        .map(|s| SlotView::new(s, state))
        .collect())
}

fn reason_code(reason: crate::domain::slot::BusyReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------- search ----------

#[derive(Deserialize)]
pub struct SearchRequest {
    range_start: DateTime<FixedOffset>,
    range_end: DateTime<FixedOffset>,
    duration_minutes: i64,
    /// "22:00" のようなローカル時刻。この時刻までに終わるスロットのみ返す
    #[serde(default)]
    latest_end_time: Option<String>,
    #[serde(default)]
    earliest_start_time: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

#[derive(Serialize)]
pub struct SearchResponse {
    slots: Vec<SlotView>,
    searched_range: SlotView,
}

async fn search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<SearchRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<SearchResponse>, ApiError> {
    authorize(&headers, &state.api_key)?;
    let Json(req) = body.map_err(|e| ApiError::bad_request("invalid_body", e.body_text()))?;

    if req.duration_minutes <= 0 {
        return Err(ApiError::bad_request(
            "invalid_duration",
            "duration_minutes は正の値で指定してください",
        ));
    }

    let range = TimeSlot::new(
        req.range_start.with_timezone(&Utc),
        req.range_end.with_timezone(&Utc),
    )
    .ok_or_else(|| {
        ApiError::bad_request(
            "invalid_range",
            "range_end は range_start より後である必要があります",
        )
    })?;

    if range.end - range.start > Duration::days(MAX_SEARCH_DAYS) {
        return Err(ApiError::bad_request(
            "range_too_wide",
            format!("探索範囲は最大{}日までです", MAX_SEARCH_DAYS),
        ));
    }

    let limit = req.limit.min(MAX_LIMIT);

    let earliest = parse_optional_time(req.earliest_start_time.as_deref(), "earliest_start_time")?;
    let latest = parse_optional_time(req.latest_end_time.as_deref(), "latest_end_time")?;

    let busy = state
        .calendar
        .fetch_busy(range)
        .await
        .map_err(ApiError::upstream)?;

    let slots = state
        .engine
        .search(range, req.duration_minutes, earliest, latest, &busy, &[], limit)
        .into_iter()
        .map(|s| SlotView::new(s, &state))
        .collect();

    Ok(Json(SearchResponse {
        slots,
        searched_range: SlotView::new(range, &state),
    }))
}

fn parse_optional_time(raw: Option<&str>, field: &str) -> Result<Option<NaiveTime>, ApiError> {
    match raw {
        None => Ok(None),
        Some(s) => parse_time(s)
            .map(Some)
            .map_err(|_| ApiError::bad_request("invalid_time", format!("{} の形式が不正です（HH:MM）", field))),
    }
}

// ---------- shared ----------

#[derive(Serialize)]
pub struct SlotView {
    start: String,
    end: String,
}

impl SlotView {
    fn new(slot: TimeSlot, state: &AppState) -> Self {
        let tz = state.engine.pattern().timezone;
        Self {
            start: slot.start.with_timezone(&tz).to_rfc3339(),
            end: slot.end.with_timezone(&tz).to_rfc3339(),
        }
    }
}

fn authorize(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    let provided = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    // 長さの違いで早期リターンせず、固定長の比較に寄せる
    if provided.len() == expected.len()
        && provided
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    {
        Ok(())
    } else {
        Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "X-API-Key が不正です".to_string(),
        })
    }
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code, message: message.into() }
    }

    fn upstream(e: anyhow::Error) -> Self {
        tracing::error!("上流APIの呼び出しに失敗: {:?}", e);
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "upstream_error",
            // 上流のエラー詳細はトークン等を含みうるため、クライアントには返さない
            message: "カレンダーの取得に失敗しました".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.code, "message": self.message })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::domain::LifePattern;
    use anyhow::Result;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const PATTERN: &str = r#"
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

    struct StubCalendar(Vec<TimeSlot>);

    #[async_trait]
    impl CalendarReader for StubCalendar {
        async fn fetch_busy(&self, _range: TimeSlot) -> Result<Vec<TimeSlot>> {
            Ok(self.0.clone())
        }
    }

    /// 実カレンダーと同じく、要求された範囲に重なる予定だけを返す。
    /// 取得範囲の取り方が誤っていると予定を取りこぼすことを検出できる
    struct RangeAwareCalendar(Vec<TimeSlot>);

    #[async_trait]
    impl CalendarReader for RangeAwareCalendar {
        async fn fetch_busy(&self, range: TimeSlot) -> Result<Vec<TimeSlot>> {
            Ok(self.0.iter().filter(|s| s.overlaps(&range)).copied().collect())
        }
    }

    /// 上流障害を模す
    struct FailingCalendar;

    #[async_trait]
    impl CalendarReader for FailingCalendar {
        async fn fetch_busy(&self, _range: TimeSlot) -> Result<Vec<TimeSlot>> {
            Err(anyhow::anyhow!("接続失敗"))
        }
    }

    fn app(busy: Vec<TimeSlot>) -> Router {
        app_with(Box::new(StubCalendar(busy)))
    }

    fn app_with(calendar: Box<dyn CalendarReader + Send + Sync>) -> Router {
        let state = Arc::new(AppState {
            calendar,
            engine: AvailabilityEngine::new(LifePattern::from_toml_str(PATTERN).unwrap()),
            api_key: "secret".to_string(),
        });
        router(state)
    }

    async fn post_json(app: Router, path: &str, key: Option<&str>, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(k) = key {
            builder = builder.header("x-api-key", k);
        }
        let response = app
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn rejects_missing_api_key() {
        let (status, _) = post_json(
            app(vec![]),
            "/v1/availability/check",
            None,
            serde_json::json!({ "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60 }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_wrong_api_key() {
        let (status, _) = post_json(
            app(vec![]),
            "/v1/availability/check",
            Some("wrong"),
            serde_json::json!({ "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60 }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn check_returns_available_for_free_evening() {
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/check",
            Some("secret"),
            serde_json::json!({ "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], true);
    }

    #[tokio::test]
    async fn check_returns_blackout_with_alternatives() {
        // 水曜11:30は勤務時間帯
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/check",
            Some("secret"),
            serde_json::json!({ "start": "2026-08-26T11:30:00+09:00", "duration_minutes": 60 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], false);
        assert_eq!(body["reason"], "blackout");
        assert!(
            !body["alternatives"].as_array().unwrap().is_empty(),
            "代替候補が返っていない"
        );
    }

    /// 実データで発生した不具合の再現:
    /// 直前に終わる予定をバッファで塞いだのに、同じ時刻を代替候補として返していた
    #[tokio::test]
    async fn alternatives_exclude_the_rejected_slot() {
        // 歯医者 16:45-18:00（8/14 金）
        let dentist = TimeSlot::new(
            chrono_tz::Asia::Tokyo.with_ymd_and_hms(2026, 8, 14, 16, 45, 0).unwrap().with_timezone(&Utc),
            chrono_tz::Asia::Tokyo.with_ymd_and_hms(2026, 8, 14, 18, 0, 0).unwrap().with_timezone(&Utc),
        )
        .unwrap();

        let (status, body) = post_json(
            app_with(Box::new(RangeAwareCalendar(vec![dentist]))),
            "/v1/availability/check",
            Some("secret"),
            serde_json::json!({ "start": "2026-08-14T18:00:00+09:00", "duration_minutes": 60 }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], false, "バッファにより塞がっているべき");

        for alt in body["alternatives"].as_array().unwrap() {
            assert_ne!(
                alt["start"].as_str().unwrap(),
                "2026-08-14T18:00:00+09:00",
                "却下した時刻を代替候補として返している"
            );
        }
    }

    #[tokio::test]
    async fn blackout_is_answered_without_calendar() {
        // カレンダーが落ちていても、生活パターンだけで決まる判定は返せる
        let (status, body) = post_json(
            app_with(Box::new(FailingCalendar)),
            "/v1/availability/check",
            Some("secret"),
            serde_json::json!({ "start": "2026-08-26T11:30:00+09:00", "duration_minutes": 60 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "上流障害で判定が落ちてはいけない");
        assert_eq!(body["available"], false);
        assert_eq!(body["reason"], "blackout");
        assert_eq!(body["detail"], "勤務");
        // 代替候補は取得できないが、判定自体は返る
        assert!(body["alternatives"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn calendar_failure_surfaces_when_verdict_needs_it() {
        // 平日夜はパターン上は空き。判定にカレンダーが要るので502が正しい
        let (status, body) = post_json(
            app_with(Box::new(FailingCalendar)),
            "/v1/availability/check",
            Some("secret"),
            serde_json::json!({ "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60 }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "upstream_error");
    }

    #[tokio::test]
    async fn check_rejects_non_positive_duration() {
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/check",
            Some("secret"),
            serde_json::json!({ "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 0 }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_duration");
    }

    #[tokio::test]
    async fn search_returns_slots_before_latest_end() {
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/search",
            Some("secret"),
            serde_json::json!({
                "range_start": "2026-08-12T00:00:00+09:00",
                "range_end": "2026-08-19T00:00:00+09:00",
                "duration_minutes": 60,
                "latest_end_time": "22:00",
                "limit": 5
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let slots = body["slots"].as_array().unwrap();
        assert!(!slots.is_empty(), "候補が返っていない");
        for s in slots {
            let end = s["end"].as_str().unwrap();
            let parsed = DateTime::parse_from_rfc3339(end).unwrap();
            assert!(
                parsed.time() <= NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                "22時を超える候補: {}",
                end
            );
        }
    }

    #[tokio::test]
    async fn search_rejects_inverted_range() {
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/search",
            Some("secret"),
            serde_json::json!({
                "range_start": "2026-08-19T00:00:00+09:00",
                "range_end": "2026-08-12T00:00:00+09:00",
                "duration_minutes": 60
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_range");
    }

    #[tokio::test]
    async fn search_rejects_too_wide_range() {
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/search",
            Some("secret"),
            serde_json::json!({
                "range_start": "2026-08-12T00:00:00+09:00",
                "range_end": "2027-08-12T00:00:00+09:00",
                "duration_minutes": 60
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "range_too_wide");
    }

    #[tokio::test]
    async fn search_rejects_malformed_time() {
        let (status, body) = post_json(
            app(vec![]),
            "/v1/availability/search",
            Some("secret"),
            serde_json::json!({
                "range_start": "2026-08-12T00:00:00+09:00",
                "range_end": "2026-08-19T00:00:00+09:00",
                "duration_minutes": 60,
                "latest_end_time": "22時"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_time");
    }

    #[tokio::test]
    async fn health_needs_no_key() {
        let response = app(vec![])
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
