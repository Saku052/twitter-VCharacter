use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait AiGenerator {
    fn generate(&self) -> Result<String>;
}
