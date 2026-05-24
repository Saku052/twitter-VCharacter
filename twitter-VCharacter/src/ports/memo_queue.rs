use anyhow::Result;
use async_trait::async_trait;

use crate::adapters::postgres::MemoRow;

#[async_trait]
pub trait MemoQueue {
    async fn fetch_latest_memo(&self) -> Result<MemoRow>;
    async fn mark_used_memo(&self, id: i32) -> Result<()>;
}
