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
    /// 書き込み側。未設定なら予約系エンドポイントは 503 を返す
    pub write: Option<crate::config::WriteSide>,
    pub engine: AvailabilityEngine,
    pub api_key: String,
}

impl AppState {
    fn write_side(&self) -> Result<&crate::config::WriteSide, ApiError> {
        self.write.as_ref().ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "write_disabled",
                "書き込み機能が無効です（TICKTICK_ACCESS_TOKEN / DATABASE_URL を設定してください）",
            )
        })
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/availability/check", post(check))
        .route("/v1/availability/search", post(search))
        .route("/v1/reservations", post(create_reservation))
        .route("/v1/reservations/{id}", axum::routing::delete(delete_reservation))
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
    let pending = pending_slots(&state, fetch_range).await;

    let response = match state.engine.check(slot, &busy, &pending) {
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
    let pending = pending_slots(state, fetch_range).await;

    Ok(state
        .engine
        .search(range, duration_minutes, None, None, &busy, &pending, 3)
        .into_iter()
        .map(|s| SlotView::new(s, state))
        .collect())
}

/// 未同期の予約を取得する。
/// 書き込み無効時や取得失敗時は空を返す（読み取り機能を巻き込んで落とさない）
async fn pending_slots(state: &Arc<AppState>, range: TimeSlot) -> Vec<TimeSlot> {
    let Some(write) = state.write.as_ref() else {
        return Vec::new();
    };
    match write.reservations.find_overlapping(range).await {
        Ok(rs) => rs.into_iter().map(|r| r.slot).collect(),
        Err(e) => {
            tracing::warn!("未同期予約の取得に失敗しました: {:?}", e);
            Vec::new()
        }
    }
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
    let pending = pending_slots(&state, range).await;

    let slots = state
        .engine
        .search(range, req.duration_minutes, earliest, latest, &busy, &pending, limit)
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

// ---------- reservations ----------

#[derive(Deserialize)]
pub struct CreateReservationRequest {
    title: String,
    start: DateTime<FixedOffset>,
    duration_minutes: i64,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    /// 登録前に空きを再確認する。既定で有効（二重予約を防ぐため）
    #[serde(default = "default_true")]
    verify_availability: bool,
    /// 呼び出し元エージェントの識別子
    #[serde(default)]
    created_by: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
pub struct CreateReservationResponse {
    reservation_id: String,
    ticktick_task_id: String,
    start: String,
    end: String,
}

async fn create_reservation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<CreateReservationRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<CreateReservationResponse>), ApiError> {
    authorize(&headers, &state.api_key)?;
    let write = state.write_side()?;
    let Json(req) = body.map_err(|e| ApiError::bad_request("invalid_body", e.body_text()))?;

    if req.duration_minutes <= 0 {
        return Err(ApiError::bad_request(
            "invalid_duration",
            "duration_minutes は正の値で指定してください",
        ));
    }
    if req.title.trim().is_empty() {
        return Err(ApiError::bad_request("invalid_title", "title は必須です"));
    }

    let slot = TimeSlot::from_duration(req.start.with_timezone(&Utc), req.duration_minutes)
        .ok_or_else(|| ApiError::bad_request("invalid_range", "有効な時間区間になりません"))?;

    if req.verify_availability {
        // カレンダーと未同期予約の両方を見る。
        // TickTick -> Google の同期には遅延があるため、DBを併せて見ないと
        // 直前に自分で入れた予定を見落として二重予約になる
        let margin = Duration::minutes(
            state.engine.pattern().buffer_before_minutes
                + state.engine.pattern().buffer_after_minutes,
        ) + Duration::hours(1);
        let fetch_range = TimeSlot::new(slot.start - margin, slot.end + margin)
            .ok_or_else(|| ApiError::bad_request("invalid_range", "取得範囲が不正です"))?;

        let busy = state
            .calendar
            .fetch_busy(fetch_range)
            .await
            .map_err(ApiError::upstream)?;
        let pending: Vec<TimeSlot> = write
            .reservations
            .find_overlapping(fetch_range)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|r| r.slot)
            .collect();

        if let Availability::Busy { reason, detail } = state.engine.check(slot, &busy, &pending) {
            let alternatives = alternatives_for(&state, slot, req.duration_minutes)
                .await
                .unwrap_or_default();
            return Err(ApiError::conflict(reason_code(reason), detail, alternatives));
        }
    }

    let created = write
        .tasks
        .create_task(crate::ports::task_writer::NewTask {
            title: req.title.clone(),
            content: req.content.clone(),
            slot,
            project_id: req.project_id.clone(),
        })
        .await
        .map_err(ApiError::upstream)?;

    // TickTickへの登録が成功した後にDBへ記録する。
    // ここで失敗するとTickTickにだけ残るため、taskIdをログに残して追跡可能にする
    let reservation_id = write
        .reservations
        .insert(crate::ports::reservation_store::NewReservation {
            task_id: created.task_id.clone(),
            project_id: created.project_id.clone(),
            title: req.title,
            slot,
            created_by: req.created_by,
        })
        .await
        .map_err(|e| {
            tracing::error!(
                ticktick_task_id = %created.task_id,
                "TickTickに登録済みだがDB保存に失敗しました。手動で確認が必要です: {:?}",
                e
            );
            ApiError::internal(e)
        })?;

    let tz = state.engine.pattern().timezone;
    Ok((
        StatusCode::CREATED,
        Json(CreateReservationResponse {
            reservation_id,
            ticktick_task_id: created.task_id,
            start: slot.start.with_timezone(&tz).to_rfc3339(),
            end: slot.end.with_timezone(&tz).to_rfc3339(),
        }),
    ))
}

async fn delete_reservation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    authorize(&headers, &state.api_key)?;
    let write = state.write_side()?;

    let Some(reservation) = write.reservations.get(&id).await.map_err(ApiError::internal)? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "予約が見つかりません",
        ));
    };

    // TickTick側を先に消す。DBだけ消えてTickTickに残る状態を避ける
    write
        .tasks
        .delete_task(&reservation.project_id, &reservation.task_id)
        .await
        .map_err(ApiError::upstream)?;

    write.reservations.cancel(&id).await.map_err(ApiError::internal)?;

    Ok(StatusCode::NO_CONTENT)
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
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "X-API-Key が不正です",
        ))
    }
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    /// 409 の際に返す代替候補
    alternatives: Vec<SlotView>,
    /// 409 の際の理由コード
    reason: Option<String>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            alternatives: Vec::new(),
            reason: None,
        }
    }

    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    fn conflict(reason: String, detail: String, alternatives: Vec<SlotView>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "slot_unavailable",
            message: detail,
            alternatives,
            reason: Some(reason),
        }
    }

    fn upstream(e: anyhow::Error) -> Self {
        tracing::error!("上流APIの呼び出しに失敗: {:?}", e);
        // 上流のエラー詳細はトークン等を含みうるため、クライアントには返さない
        Self::new(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            "外部サービスの呼び出しに失敗しました",
        )
    }

    fn internal(e: anyhow::Error) -> Self {
        tracing::error!("内部エラー: {:?}", e);
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "内部エラーが発生しました",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "error": self.code, "message": self.message });
        if let Some(reason) = self.reason {
            body["reason"] = serde_json::Value::String(reason);
        }
        if !self.alternatives.is_empty() {
            body["alternatives"] = serde_json::to_value(&self.alternatives).unwrap_or_default();
        }
        (self.status, Json(body)).into_response()
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
            write: None,
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

    // ---------- Phase B: 予約 ----------

    use crate::ports::reservation_store::{NewReservation, Reservation, ReservationStore};
    use crate::ports::task_writer::{CreatedTask, NewTask, TaskWriter};
    use std::sync::Mutex;

    #[derive(Default)]
    struct StubTasks {
        created: Mutex<Vec<NewTask>>,
        deleted: Mutex<Vec<(String, String)>>,
        fail: bool,
    }

    #[async_trait]
    impl TaskWriter for StubTasks {
        async fn create_task(&self, req: NewTask) -> Result<CreatedTask> {
            if self.fail {
                anyhow::bail!("TickTick障害");
            }
            self.created.lock().unwrap().push(req);
            Ok(CreatedTask {
                task_id: "task-1".into(),
                project_id: "proj-1".into(),
            })
        }
        async fn delete_task(&self, p: &str, t: &str) -> Result<()> {
            self.deleted.lock().unwrap().push((p.into(), t.into()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct StubStore {
        rows: Mutex<Vec<Reservation>>,
    }

    #[async_trait]
    impl ReservationStore for StubStore {
        async fn insert(&self, r: NewReservation) -> Result<String> {
            let id = format!("res-{}", self.rows.lock().unwrap().len() + 1);
            self.rows.lock().unwrap().push(Reservation {
                id: id.clone(),
                task_id: r.task_id,
                project_id: r.project_id,
                title: r.title,
                slot: r.slot,
            });
            Ok(id)
        }
        async fn find_overlapping(&self, range: TimeSlot) -> Result<Vec<Reservation>> {
            Ok(self.rows.lock().unwrap().iter()
                .filter(|r| r.slot.overlaps(&range)).cloned().collect())
        }
        async fn get(&self, id: &str) -> Result<Option<Reservation>> {
            Ok(self.rows.lock().unwrap().iter().find(|r| r.id == id).cloned())
        }
        async fn cancel(&self, id: &str) -> Result<bool> {
            let mut rows = self.rows.lock().unwrap();
            let before = rows.len();
            rows.retain(|r| r.id != id);
            Ok(rows.len() < before)
        }
    }

    fn app_writable(busy: Vec<TimeSlot>) -> Router {
        let state = Arc::new(AppState {
            calendar: Box::new(RangeAwareCalendar(busy)),
            write: Some(crate::config::WriteSide {
                tasks: Box::new(StubTasks::default()),
                reservations: Box::new(StubStore::default()),
            }),
            engine: AvailabilityEngine::new(LifePattern::from_toml_str(PATTERN).unwrap()),
            api_key: "secret".to_string(),
        });
        router(state)
    }

    #[tokio::test]
    async fn reservation_is_created_when_slot_is_free() {
        let (status, body) = post_json(
            app_writable(vec![]),
            "/v1/reservations",
            Some("secret"),
            serde_json::json!({
                "title": "打ち合わせ",
                "start": "2026-08-26T19:00:00+09:00",
                "duration_minutes": 60
            }),
        ).await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["ticktick_task_id"], "task-1");
        assert!(body["reservation_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn reservation_is_rejected_during_work_hours() {
        let (status, body) = post_json(
            app_writable(vec![]),
            "/v1/reservations",
            Some("secret"),
            serde_json::json!({
                "title": "x",
                "start": "2026-08-26T11:30:00+09:00",
                "duration_minutes": 60
            }),
        ).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "slot_unavailable");
        assert_eq!(body["reason"], "blackout");
        assert!(!body["alternatives"].as_array().unwrap().is_empty(), "代替候補を返すべき");
    }

    /// 同期遅延中の二重予約を防げること。これがPhase Bの肝
    #[tokio::test]
    async fn second_reservation_on_same_slot_is_rejected() {
        let app = app_writable(vec![]);

        let (s1, _) = post_json(
            app.clone(), "/v1/reservations", Some("secret"),
            serde_json::json!({
                "title": "1件目", "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60
            }),
        ).await;
        assert_eq!(s1, StatusCode::CREATED);

        // カレンダー未反映でも、DBの記録により塞がりと判定されるべき
        let (s2, body) = post_json(
            app, "/v1/reservations", Some("secret"),
            serde_json::json!({
                "title": "2件目", "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60
            }),
        ).await;

        assert_eq!(s2, StatusCode::CONFLICT, "同期遅延中の二重予約を許してはいけない");
        assert_eq!(body["reason"], "busy_pending");
    }

    #[tokio::test]
    async fn check_sees_pending_reservation() {
        let app = app_writable(vec![]);
        post_json(
            app.clone(), "/v1/reservations", Some("secret"),
            serde_json::json!({
                "title": "予約済み", "start": "2026-08-26T20:00:00+09:00", "duration_minutes": 60
            }),
        ).await;

        let (status, body) = post_json(
            app, "/v1/availability/check", Some("secret"),
            serde_json::json!({ "start": "2026-08-26T20:00:00+09:00", "duration_minutes": 60 }),
        ).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], false, "自分で入れた予約を見落としている");
        assert_eq!(body["reason"], "busy_pending");
    }

    #[tokio::test]
    async fn reservation_rejects_empty_title() {
        let (status, body) = post_json(
            app_writable(vec![]), "/v1/reservations", Some("secret"),
            serde_json::json!({
                "title": "   ", "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60
            }),
        ).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_title");
    }

    #[tokio::test]
    async fn reservation_requires_api_key() {
        let (status, _) = post_json(
            app_writable(vec![]), "/v1/reservations", None,
            serde_json::json!({
                "title": "x", "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60
            }),
        ).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn reservations_disabled_without_write_side() {
        let (status, body) = post_json(
            app(vec![]), "/v1/reservations", Some("secret"),
            serde_json::json!({
                "title": "x", "start": "2026-08-26T19:00:00+09:00", "duration_minutes": 60
            }),
        ).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"], "write_disabled");
    }

    #[tokio::test]
    async fn verify_availability_false_skips_the_check() {
        // 勤務時間中でも明示的に無効化すれば登録できる
        let (status, _) = post_json(
            app_writable(vec![]), "/v1/reservations", Some("secret"),
            serde_json::json!({
                "title": "強制登録",
                "start": "2026-08-26T11:30:00+09:00",
                "duration_minutes": 60,
                "verify_availability": false
            }),
        ).await;
        assert_eq!(status, StatusCode::CREATED);
    }
}
