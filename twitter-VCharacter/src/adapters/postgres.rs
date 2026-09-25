use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;
use chrono::{DateTime, Utc};
use crate::domain::schedule::{PlannedSlot, SCHEDULE_VERSION};
use crate::ports::memo_queue::MemoQueue;
use crate::ports::schedule_store::{DueSlot, ScheduleStore};

pub struct PostgresClient {
    pool: PgPool,
    // TODO: ここに momo {String, id}入れたとして、一つのインスタンスとして残るのか検証してみる
}

impl PostgresClient {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self { pool })
    }
}

#[derive(sqlx::FromRow)]
pub struct MemoRow {
    pub id: i32,
    pub memo: Option<String>,
    /// メモの出所（agent / qiita / youtube）。どの経路のネタが伸びるかの分析に使う
    pub source: Option<String>,
}

// TODO: FronRowトレイト for {variable}ってことだよねderiveって。
// それがしたいのはMemoRowの変数にマッピングするfunctionがFromRowに備わっているから
// じゃあMemoQueueトレイトをPostgresClientにした理由ってなに？これを調査してまとめる
#[async_trait]
impl MemoQueue for PostgresClient {
    async fn fetch_latest_memo(&self) -> Result<MemoRow> {
        let row = sqlx::query_as!(MemoRow,
            "SELECT
                id, memo, source
            FROM
                memo_mq
            WHERE
                used_at IS NULL
                AND skipped_reason IS NULL
            ORDER BY
                created_at, id
            LIMIT 1"
        )
        .fetch_one(&self.pool)
        .await?; 

        Ok(row) // TODO: Resultのベクトル化
    }

    async fn mark_used_memo(&self, id: i32) -> Result<()> {
        sqlx::query!(
            "UPDATE memo_mq
            SET used_at = NOW()
            WHERE id = $1",
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_skipped_memo(&self, id: i32, reason: &str) -> Result<()> {
        sqlx::query!(
            "UPDATE memo_mq
            SET skipped_reason = $2
            WHERE id = $1",
            id,
            reason
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_posted_tweet(
        &self,
        tweet_id: &str,
        memo_id: i32,
        body: &str,
        tags: &str,
        source: Option<&str>,
    ) -> Result<()> {
        // 同じ tweet_id を二重に記録しない（再実行時の保険）
        sqlx::query!(
            "INSERT INTO posted_tweets (tweet_id, memo_id, body, tags, source)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (tweet_id) DO NOTHING",
            tweet_id,
            memo_id,
            body,
            tags,
            source
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl ScheduleStore for PostgresClient {
    async fn save_plan(&self, slots: &[PlannedSlot]) -> Result<usize> {
        let mut saved = 0usize;
        for s in slots {
            // 同じ時刻の予定が既にあれば無視する（planner の再実行に耐える）
            let r = sqlx::query!(
                "INSERT INTO post_schedule
                    (planned_at, block, planned_count, seq_in_day, schedule_ver)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (planned_at) DO NOTHING",
                s.planned_at.with_timezone(&Utc),
                s.block,
                s.planned_count as i32,
                s.seq_in_day as i32,
                SCHEDULE_VERSION,
            )
            .execute(&self.pool)
            .await?;
            saved += r.rows_affected() as usize;
        }
        Ok(saved)
    }

    async fn claim_due_slot(&self, now: DateTime<Utc>) -> Result<Option<DueSlot>> {
        // 取り出しと同時に running にする。複数の tick が重なっても同じ予定を二重に拾わない。
        // 期限を大きく過ぎたものは投稿せず捨てる（遅れて出すと時刻の記録と実態がずれるため）
        let row = sqlx::query!(
            "UPDATE post_schedule
             SET status = 'running'
             WHERE id = (
                 SELECT id FROM post_schedule
                 WHERE status = 'pending'
                   AND planned_at <= $1
                   AND planned_at > $1 - INTERVAL '30 minutes'
                 ORDER BY planned_at
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING id, planned_at, block, planned_count, seq_in_day, schedule_ver",
            now,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| DueSlot {
            id: r.id,
            planned_at: r.planned_at,
            block: r.block,
            planned_count: r.planned_count,
            seq_in_day: r.seq_in_day,
            schedule_ver: r.schedule_ver,
        }))
    }

    async fn complete_slot(&self, id: i32, tweet_id: &str) -> Result<()> {
        sqlx::query!(
            "UPDATE post_schedule
             SET status = 'done', tweet_id = $2, executed_at = NOW()
             WHERE id = $1",
            id,
            tweet_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn release_slot(&self, id: i32, status: &str) -> Result<()> {
        sqlx::query!(
            "UPDATE post_schedule
             SET status = $2, executed_at = NOW()
             WHERE id = $1",
            id,
            status,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn count_pending_after(&self, from: DateTime<Utc>) -> Result<i64> {
        let row = sqlx::query!(
            "SELECT count(*) AS n FROM post_schedule
             WHERE status = 'pending' AND planned_at >= $1",
            from,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.n.unwrap_or(0))
    }
}
