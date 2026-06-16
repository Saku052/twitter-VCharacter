use async_trait::async_trait;
use anyhow::Result;

#[async_trait]
pub trait YoutubePort {
    async fn get_youtube_video(&self) -> Result<(String, String)>;
}