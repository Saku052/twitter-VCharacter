use async_trait::async_trait;
use anyhow::Result;
use crate::domain::QiitaArticle;

#[async_trait]
pub trait QiitaPort {
    async fn fetch_trending_articles(&self) -> Result<Vec<QiitaArticle>>;
}
