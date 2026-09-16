use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TextPublisher {
    /// 投稿に成功したら X API が採番した tweet_id を返す。
    /// 反響分析は後から `GET /2/tweets/{id}` で指標を引くため、この id が唯一の鍵になる。
    async fn post_text(&self, content: &str, media_ids: Option<Vec<String>>) -> Result<String>;
}
