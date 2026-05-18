use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait MemoQueue {
    async fn fetch_latest_memo(&self) -> Result<String>;
    // TODO: posted_atというカラムをアップデートするポートを作成
}
