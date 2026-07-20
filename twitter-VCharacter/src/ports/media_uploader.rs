use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait MediaUploader {
    async fn upload_media(&self, image: &[u8]) -> Result<String>;
}
