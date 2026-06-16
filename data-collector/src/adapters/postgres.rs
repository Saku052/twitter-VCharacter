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
    async fn insert_memo(&self, memo: &str) -> Result<()> {
        sqlx::query!("INSERT INTO memo_mq (memo) VALUES ($1)", memo)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
