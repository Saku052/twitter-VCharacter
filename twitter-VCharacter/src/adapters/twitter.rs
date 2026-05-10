use anyhow::Result;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac, KeyInit};
use reqwest::Client;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::ports::text_publisher::TextPublisher;

type HmacSha256 = Hmac<Sha256>;

pub struct TwitterClient {
    api_key: String,
    api_secret: String,
    access_token: String,
    access_token_secret: String,
    client: Client,
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
    async fn post_text(&self, content: &str) -> Result<()> {
        let url = "https://api.twitter.com/2/tweets";
        let auth_header = self.build_oauth_header("POST", url)?;
        let body = serde_json::json!({ "text": content });

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
