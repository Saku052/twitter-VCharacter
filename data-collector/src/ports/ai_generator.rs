use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait AiGenerator {
    async fn generate(&self, memo: &str, model: &str, system: &str) -> Result<String>;
}
