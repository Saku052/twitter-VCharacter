use anyhow::Result;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use crate::ports::ai_generator::AiGenerator;
use crate::ports::image_generator::ImageGenerator;

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const OPENAI_IMAGE_API_URL: &str = "https://api.openai.com/v1/images/generations";
const IMAGE_MODEL: &str = "gpt-image-2";

pub struct OpenAiClient {
    api_key: String,
    image_api_url: String,
}

impl OpenAiClient {
    pub fn new(api_key: String) -> Self {
        Self { api_key, image_api_url: OPENAI_IMAGE_API_URL.to_string() }
    }

    #[cfg(test)]
    fn with_image_api_url(api_key: String, image_api_url: String) -> Self {
        Self { api_key, image_api_url }
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

#[async_trait]
impl ImageGenerator for OpenAiClient {
    async fn generate_image(&self, prompt: &str) -> Result<Vec<u8>> {
        let client = reqwest::Client::new();

        let response = client
            .post(&self.image_api_url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": IMAGE_MODEL,
                "prompt": prompt,
                "size": "1024x1024",
                "quality": "low",
                "n": 1
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            anyhow::bail!("画像生成失敗 ({}): {}", status, text);
        }

        let body: serde_json::Value = response.json().await?;
        let b64 = body["data"][0]["b64_json"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("OpenAI画像生成のレスポンスが不正です"))?;

        let image_bytes = STANDARD.decode(b64)?;
        Ok(image_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::method;

    #[tokio::test]
    async fn generate_image_decodes_base64_response_into_bytes() {
        let mock_server = MockServer::start().await;

        let original_bytes = b"fake-png-bytes";
        let b64 = STANDARD.encode(original_bytes);
        let response_body = serde_json::json!({
            "data": [ { "b64_json": b64 } ]
        });

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::with_image_api_url("dummy-key".to_string(), mock_server.uri());
        let image_bytes = client.generate_image("テストプロンプト").await.unwrap();

        assert_eq!(image_bytes, original_bytes.to_vec());
    }

    #[tokio::test]
    async fn generate_image_errors_when_b64_json_field_is_missing() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({ "data": [ {} ] });

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::with_image_api_url("dummy-key".to_string(), mock_server.uri());
        let result = client.generate_image("テストプロンプト").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn generate_image_errors_on_non_success_status() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::with_image_api_url("dummy-key".to_string(), mock_server.uri());
        let result = client.generate_image("テストプロンプト").await;

        assert!(result.is_err());
    }
}
