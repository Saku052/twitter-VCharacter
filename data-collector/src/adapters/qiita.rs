use async_trait::async_trait;
use serde::Deserialize;
use chrono::{Duration, Utc};
use crate::ports::qiita_port::QiitaPort;
use crate::domain::QiitaArticle;
use anyhow::Result;

const DEFAULT_BASE_URL: &str = "https://qiita.com/api/v2";

pub struct QiitaClient {
    base_url: String,
}

impl QiitaClient {
    pub fn new() -> Self {
        Self { base_url: DEFAULT_BASE_URL.to_string() }
    }

    #[cfg(test)]
    fn with_base_url(base_url: String) -> Self {
        Self { base_url }
    }
}

#[async_trait]
impl QiitaPort for QiitaClient {

    async fn fetch_trending_articles(&self) -> Result<Vec<QiitaArticle>> {
        let client = reqwest::Client::new();

        let thirty_days_ago = (Utc::now() - Duration::days(30)).format("%Y-%m-%d").to_string();
        let query = format!("tag:ゲーム開発 stocks:>1 created:>{}", thirty_days_ago);

        let response = client
            .get(format!("{}/items", self.base_url))
            .query(&[
                ("query", query.as_str()),
                ("page", "1"),
                // 取得段階では絞らない。ここを小さくすると、上位が処理済みで埋まったとき
                // 未処理の記事に永久に到達できなくなる（実測: 30日中7日しか取得できず）。
                // 1回あたりの投入上限はmain.rs側のQIITA_MAX_PER_RUNで掛ける。
                ("per_page", "20"),
            ])
            .send()
            .await?
            .error_for_status()?;

        let items: Vec<QiitaItem> = response.json().await?;
        let articles = items
            .into_iter()
            .map(|item| QiitaArticle {
                article_id: item.id,
                title: item.title,
                body_excerpt: item.body.chars().take(300).collect::<String>(),
            })
            .collect();

        Ok(articles)
    }

}

//----------------------------------------------------------------------------------------------------------------------------
//Qiita JSON Response
//----------------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct QiitaItem {
    id: String,
    title: String,
    body: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    #[tokio::test]
    async fn truncates_body_to_300_chars_without_panicking_on_multibyte_boundary() {
        let mock_server = MockServer::start().await;

        let long_body: String = "あ".repeat(500);
        let response_body = serde_json::json!([
            {
                "id": "test-article-1",
                "title": "テスト記事",
                "body": long_body
            }
        ]);

        Mock::given(method("GET"))
            .and(path("/items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = QiitaClient::with_base_url(mock_server.uri());
        let articles = client.fetch_trending_articles().await.unwrap();

        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].body_excerpt.chars().count(), 300);
    }

    #[tokio::test]
    async fn keeps_body_as_is_when_shorter_than_300_chars() {
        let mock_server = MockServer::start().await;
        let short_body = "短い本文です。".to_string();
        let response_body = serde_json::json!([
            { "id": "test-article-2", "title": "短い記事", "body": short_body.clone() }
        ]);

        Mock::given(method("GET"))
            .and(path("/items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = QiitaClient::with_base_url(mock_server.uri());
        let articles = client.fetch_trending_articles().await.unwrap();

        assert_eq!(articles[0].body_excerpt, short_body);
    }
}
