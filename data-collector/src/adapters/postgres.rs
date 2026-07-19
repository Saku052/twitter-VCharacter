use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;
use crate::ports::memo_writer::MemoWriter;

pub struct PostgresClient {
    pool: PgPool,
}

impl PostgresClient {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl MemoWriter for PostgresClient {
    async fn insert_memo(&self, memo: &str, video_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "INSERT INTO processed_videos (video_id) VALUES ($1) ON CONFLICT DO NOTHING",
            video_id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO memo_mq (memo, source) VALUES ($1, 'youtube')",
            memo
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn is_processed(&self, video_id: &str) -> Result<bool> {
        let row = sqlx::query!(
            "SELECT EXISTS(SELECT 1 FROM processed_videos WHERE video_id = $1) AS \"exists!\"",
            video_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.exists)
    }

    async fn insert_qiita_memo(&self, memo: &str, article_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "INSERT INTO processed_qiita_items (article_id) VALUES ($1) ON CONFLICT DO NOTHING",
            article_id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO memo_mq (memo, source) VALUES ($1, 'qiita')",
            memo
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn is_qiita_processed(&self, article_id: &str) -> Result<bool> {
        let row = sqlx::query!(
            "SELECT EXISTS(SELECT 1 FROM processed_qiita_items WHERE article_id = $1) AS \"exists!\"",
            article_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.exists)
    }

    async fn insert_agent_memo(&self, memo: &str) -> Result<()> {
        sqlx::query!(
            "INSERT INTO memo_mq (memo, source) VALUES ($1, 'agent')",
            memo
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
