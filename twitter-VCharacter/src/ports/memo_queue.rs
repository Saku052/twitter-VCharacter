use anyhow::Result;
use async_trait::async_trait;

use crate::adapters::postgres::MemoRow;

#[async_trait]
pub trait MemoQueue {
    async fn fetch_latest_memo(&self) -> Result<MemoRow>;
    // TODO: posted_atというカラムをアップデートするポートを作成
}
