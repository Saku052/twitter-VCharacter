use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TextPublisher {
    async fn post_text(&self, content: &str, media_ids: Option<Vec<String>>) -> Result<()>;
}
