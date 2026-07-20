use anyhow::Result;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac, KeyInit};
use reqwest::multipart;
use reqwest::Client;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::ports::media_uploader::MediaUploader;
use crate::ports::text_publisher::TextPublisher;

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_MEDIA_UPLOAD_URL: &str = "https://api.x.com/2/media/upload";

pub struct TwitterClient {
    api_key: String,
    api_secret: String,
    access_token: String,
    access_token_secret: String,
    client: Client,
    media_upload_url: String,
}

impl TwitterClient {
    pub fn new(
        api_key: String,
        api_secret: String,
        access_token: String,
        access_token_secret: String,
    ) -> Self {
        Self {
            api_key,
            api_secret,
            access_token,
            access_token_secret,
            client: Client::new(),
            media_upload_url: DEFAULT_MEDIA_UPLOAD_URL.to_string(),
        }
    }

    #[cfg(test)]
    fn with_media_upload_url(
        api_key: String,
        api_secret: String,
        access_token: String,
        access_token_secret: String,
        media_upload_url: String,
    ) -> Self {
        Self {
            api_key,
            api_secret,
            access_token,
            access_token_secret,
            client: Client::new(),
            media_upload_url,
        }
    }

    fn build_oauth_header(&self, method: &str, url: &str) -> Result<String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let nonce = format!("{:x}", rand_nonce());

        let mut params = BTreeMap::new();
        params.insert("oauth_consumer_key", self.api_key.as_str());
        params.insert("oauth_nonce", &nonce);
        params.insert("oauth_signature_method", "HMAC-SHA256");
        params.insert("oauth_timestamp", &timestamp);
        params.insert("oauth_token", self.access_token.as_str());
        params.insert("oauth_version", "1.0");

        let params_str = params
            .iter()
            .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let base_string = format!(
            "{}&{}&{}",
            method,
            percent_encode(url),
            percent_encode(&params_str)
        );

        let signing_key = format!(
            "{}&{}",
            percent_encode(&self.api_secret),
            percent_encode(&self.access_token_secret)
        );

        let mut mac = HmacSha256::new_from_slice(signing_key.as_bytes())?;
        mac.update(base_string.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        let header = format!(
            r#"OAuth oauth_consumer_key="{}", oauth_nonce="{}", oauth_signature="{}", oauth_signature_method="HMAC-SHA256", oauth_timestamp="{}", oauth_token="{}", oauth_version="1.0""#,
            self.api_key,
            nonce,
            percent_encode(&signature),
            timestamp,
            self.access_token,
        );

        Ok(header)
    }
}

#[async_trait]
impl TextPublisher for TwitterClient {
    async fn post_text(&self, content: &str, media_ids: Option<Vec<String>>) -> Result<()> {
        let url = "https://api.twitter.com/2/tweets";
        let auth_header = self.build_oauth_header("POST", url)?;

        let mut body = serde_json::json!({ "text": content });
        if let Some(ids) = media_ids {
            body["media"] = serde_json::json!({ "media_ids": ids });
        }

        let response = self
            .client
            .post(url)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            println!("投稿成功: {}", content);
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await?;
            anyhow::bail!("投稿失敗 ({}): {}", status, text);
        }
    }
}

#[async_trait]
impl MediaUploader for TwitterClient {
    async fn upload_media(&self, image: &[u8]) -> Result<String> {
        let url = &self.media_upload_url;
        let auth_header = self.build_oauth_header("POST", url)?;

        let part = multipart::Part::bytes(image.to_vec()).file_name("image.png");
        let form = multipart::Form::new()
            .text("media_category", "tweet_image")
            .part("media", part);

        let response = self
            .client
            .post(url)
            .header("Authorization", auth_header)
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            anyhow::bail!("画像アップロード失敗 ({}): {}", status, text);
        }

        let body: serde_json::Value = response.json().await?;
        let media_id = body["data"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("media/uploadのレスポンスが不正です"))?
            .to_string();

        Ok(media_id)
    }
}

fn percent_encode(s: &str) -> String {
    let mut encoded = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

fn rand_nonce() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate, Request};
    use wiremock::matchers::{method, header_exists};

    fn test_client(media_upload_url: String) -> TwitterClient {
        TwitterClient::with_media_upload_url(
            "api-key".to_string(),
            "api-secret".to_string(),
            "access-token".to_string(),
            "access-token-secret".to_string(),
            media_upload_url,
        )
    }

    #[tokio::test]
    async fn upload_media_sends_multipart_body_and_returns_media_id() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(header_exists("Authorization"))
            .respond_with(|req: &Request| {
                let body_str = String::from_utf8_lossy(&req.body).to_string();
                assert!(body_str.contains("name=\"media_category\""), "media_categoryフィールドが送られていない");
                assert!(body_str.contains("tweet_image"), "media_categoryの値がtweet_imageでない");
                assert!(body_str.contains("name=\"media\""), "mediaパートが送られていない");
                assert!(
                    req.headers.get("authorization").is_some(),
                    "Authorizationヘッダがない"
                );
                let auth_value = req.headers.get("authorization").unwrap().to_str().unwrap();
                assert!(auth_value.starts_with("OAuth "), "OAuth形式のAuthorizationヘッダでない");

                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": { "id": "1234567890" }
                }))
            })
            .mount(&mock_server)
            .await;

        let client = test_client(mock_server.uri());
        let media_id = client.upload_media(b"fake-image-bytes").await.unwrap();

        assert_eq!(media_id, "1234567890");
    }

    #[tokio::test]
    async fn upload_media_errors_when_media_id_field_is_missing() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": {} })))
            .mount(&mock_server)
            .await;

        let client = test_client(mock_server.uri());
        let result = client.upload_media(b"fake-image-bytes").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn upload_media_errors_on_non_success_status() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(413).set_body_string("payload too large"))
            .mount(&mock_server)
            .await;

        let client = test_client(mock_server.uri());
        let result = client.upload_media(b"fake-image-bytes").await;

        assert!(result.is_err());
    }
}
