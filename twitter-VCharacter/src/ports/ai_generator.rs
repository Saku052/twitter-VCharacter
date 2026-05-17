use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait AiGenerator {
    async fn generate(&self, memo: &str, system: &str, model: &str) -> Result<String>;
}
