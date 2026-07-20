use async_trait::async_trait;
use anyhow::Result;
use crate::ports::agent_port::AgentPort;

pub struct AgentClient {
    base_url: String,
    api_key: String,
}

impl AgentClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self { base_url, api_key }
    }
}

#[async_trait]
impl AgentPort for AgentClient {
    async fn investigate(&self) -> Result<Vec<String>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(1200))
            .build()?;

        let response = client
            .post(format!("{}/investigate", self.base_url))
            .header("X-API-Key", &self.api_key)
            .send()
            .await?
            .error_for_status()?;

        let body: InvestigateResponse = response.json().await?;
        Ok(body.memos)
    }
}

#[derive(serde::Deserialize)]
struct InvestigateResponse {
    memos: Vec<String>,
}
