use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait MemoWriter {
    async fn insert_memo(&self, memo: &str, video_id: &str) -> Result<()>;
    async fn is_processed(&self, video_id: &str) -> Result<bool>;
    async fn insert_qiita_memo(&self, memo: &str, article_id: &str) -> Result<()>;
    async fn is_qiita_processed(&self, article_id: &str) -> Result<bool>;
    async fn insert_agent_memo(&self, memo: &str) -> Result<()>;
}
