use anyhow::Result;
use async_trait::async_trait;
use crate::ports::ai_generator::AiGenerator;

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const USER_PROMPT: &str = "今日のツイートを生成してください。";

pub struct OpenAiClient {
    api_key: String,
    model: String,
    system_prompt: String,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String, system_prompt: String) -> Self {
        Self { api_key, model, system_prompt }
    }
}

#[async_trait]
impl AiGenerator for OpenAiClient {
    async fn generate(&self) -> Result<String> {
        let client = reqwest::Client::new();

        let response = client
            .post(OPENAI_API_URL)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": self.system_prompt
                    },
                    {
                        "role": "user",
                        "content": USER_PROMPT
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
