use async_trait::async_trait;
use anyhow::Result;

#[async_trait]
pub trait AgentPort {
    async fn investigate(&self) -> Result<Vec<String>>;
}
