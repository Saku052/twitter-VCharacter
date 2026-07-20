use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait ImageGenerator {
    async fn generate_image(&self, prompt: &str) -> Result<Vec<u8>>;
}
