use async_trait::async_trait;
use anyhow::Result;
use crate::domain::VideoInfo;

#[async_trait]
pub trait YoutubePort {
    async fn fetch_recent_videos(&self) -> Result<Vec<VideoInfo>>;
}
