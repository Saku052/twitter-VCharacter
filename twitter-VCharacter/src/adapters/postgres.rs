use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;
use crate::ports::memo_queue::MemoQueue;

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
}

// TODO: FronRowトレイト for {variable}ってことだよねderiveって。
// それがしたいのはMemoRowの変数にマッピングするfunctionがFromRowに備わっているから
// じゃあMemoQueueトレイトをPostgresClientにした理由ってなに？これを調査してまとめる
#[async_trait]
impl MemoQueue for PostgresClient {
    async fn fetch_latest_memo(&self) -> Result<MemoRow> {
        let row = sqlx::query_as!(MemoRow,
            "SELECT
                id, memo
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
}
