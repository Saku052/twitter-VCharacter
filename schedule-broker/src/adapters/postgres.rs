use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::TimeSlot;
use crate::ports::reservation_store::{NewReservation, Reservation, ReservationStore};

pub struct PostgresClient {
    pool: PgPool,
}

impl PostgresClient {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("Postgresへの接続に失敗しました")?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl ReservationStore for PostgresClient {
    async fn insert(&self, r: NewReservation) -> Result<String> {
        // sqlx::query! はビルド時にDB接続を要するため、実行時検証の query を使う。
        // 本コンポーネントは他2つと違い .sqlx キャッシュを持たない
        let row = sqlx::query(
            "INSERT INTO reservations
                 (ticktick_task_id, ticktick_project_id, title, starts_at, ends_at, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id",
        )
        .bind(&r.task_id)
        .bind(&r.project_id)
        .bind(&r.title)
        .bind(r.slot.start)
        .bind(r.slot.end)
        .bind(&r.created_by)
        .fetch_one(&self.pool)
        .await
        .context("予約の保存に失敗しました")?;

        let id: Uuid = row.try_get("id")?;
        Ok(id.to_string())
    }

    async fn find_overlapping(&self, range: TimeSlot) -> Result<Vec<Reservation>> {
        // 半開区間 [start, end) での重なり判定。隣接は重ならない
        let rows = sqlx::query(
            "SELECT id, ticktick_task_id, ticktick_project_id, title, starts_at, ends_at
             FROM reservations
             WHERE status = 'confirmed' AND starts_at < $2 AND ends_at > $1
             ORDER BY starts_at",
        )
        .bind(range.start)
        .bind(range.end)
        .fetch_all(&self.pool)
        .await
        .context("予約の取得に失敗しました")?;

        let mut out = Vec::new();
        for row in rows {
            let start = row.try_get("starts_at")?;
            let end = row.try_get("ends_at")?;
            let Some(slot) = TimeSlot::new(start, end) else {
                // 長さ0以下の行は判定に使えないので飛ばす
                tracing::warn!("不正な区間の予約を無視しました: {} - {}", start, end);
                continue;
            };
            let id: Uuid = row.try_get("id")?;
            out.push(Reservation {
                id: id.to_string(),
                task_id: row.try_get("ticktick_task_id")?,
                project_id: row.try_get("ticktick_project_id")?,
                title: row.try_get("title")?,
                slot,
            });
        }
        Ok(out)
    }

    async fn get(&self, id: &str) -> Result<Option<Reservation>> {
        let Ok(uuid) = Uuid::parse_str(id) else {
            return Ok(None);
        };

        let row = sqlx::query(
            "SELECT id, ticktick_task_id, ticktick_project_id, title, starts_at, ends_at
             FROM reservations
             WHERE id = $1 AND status = 'confirmed'",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await
        .context("予約の取得に失敗しました")?;

        let Some(row) = row else { return Ok(None) };

        let start = row.try_get("starts_at")?;
        let end = row.try_get("ends_at")?;
        let Some(slot) = TimeSlot::new(start, end) else {
            return Ok(None);
        };
        let rid: Uuid = row.try_get("id")?;

        Ok(Some(Reservation {
            id: rid.to_string(),
            task_id: row.try_get("ticktick_task_id")?,
            project_id: row.try_get("ticktick_project_id")?,
            title: row.try_get("title")?,
            slot,
        }))
    }

    async fn cancel(&self, id: &str) -> Result<bool> {
        let Ok(uuid) = Uuid::parse_str(id) else {
            return Ok(false);
        };

        let result = sqlx::query(
            "UPDATE reservations SET status = 'cancelled'
             WHERE id = $1 AND status = 'confirmed'",
        )
        .bind(uuid)
        .execute(&self.pool)
        .await
        .context("予約の取り消しに失敗しました")?;

        Ok(result.rows_affected() > 0)
    }
}
