use anyhow::Result;
use async_trait::async_trait;
use crate::ports::ai_generator::AiGenerator;

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAiClient {
    api_key: String,
}

impl OpenAiClient {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl AiGenerator for OpenAiClient {
    async fn generate(&self, memo: &str, model: &str, system: &str) -> Result<String> {
        let client = reqwest::Client::new();

        let response = client
            .post(OPENAI_API_URL)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": model.to_string(),
                "messages": [
                    {
                        "role": "system",
                        "content": system.to_string()
                    },
                    {
                        "role": "user",
                        "content": memo.to_string()
                    }
                ]
            }))
            .send()
            .await?;

        let body: serde_json::Value = response.json().await?;
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("OpenAIのレスポンスが不正です"))?
            .to_string();

        Ok(content)
    }
}
