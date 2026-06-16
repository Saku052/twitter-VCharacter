use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait MemoQueue {
    async fn insert_memo(&self, memo: &str) -> Result<()>;
}
