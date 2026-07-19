use async_trait::async_trait;
use serde::Deserialize;
use chrono::{Duration, Utc};
use crate::ports::qiita_port::QiitaPort;
use crate::domain::QiitaArticle;
use anyhow::Result;

pub struct QiitaClient;

impl QiitaClient {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl QiitaPort for QiitaClient {

    async fn fetch_trending_articles(&self) -> Result<Vec<QiitaArticle>> {
        let client = reqwest::Client::new();

        let yesterday = (Utc::now() - Duration::days(1)).format("%Y-%m-%d").to_string();
        let query = format!("stocks:>20 stocks:<50 created:>{}", yesterday);

        let response = client
            .get("https://qiita.com/api/v2/items")
            .query(&[
                ("query", query.as_str()),
                ("page", "1"),
                ("per_page", "100"),
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
