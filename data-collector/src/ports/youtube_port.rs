use async_trait::async_trait;
use anyhow::Result;

#[async_trait]
pub trait youtube_port {
    async fn get_youtube_video(&self) -> Result<(String, String)>;
}