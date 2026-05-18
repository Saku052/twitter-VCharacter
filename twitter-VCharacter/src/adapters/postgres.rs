use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;
use crate::ports::memo_queue::MemoQueue;

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
impl MemoQueue for PostgresClient {
    async fn fetch_latest_memo(&self) -> Result<String> {
        let memo = sqlx::query_scalar::<_, String>(
            "SELECT
                memo
            FROM
                memo_mq
            WHERE
                used_at IS NULL
            ORDER BY
                created_at
            LIMIT 1"
        )
        .fetch_one(&self.pool)
        .await?;
        //　この後テーブルをアップデートしないといけないよね

        Ok(memo)
    }
}
