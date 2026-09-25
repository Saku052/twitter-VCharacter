use anyhow::Result;
use async_trait::async_trait;

use crate::adapters::postgres::MemoRow;

#[async_trait]
pub trait MemoQueue {
    /// 未使用かつ未スキップのメモのうち最も古い1件
    async fn fetch_latest_memo(&self) -> Result<MemoRow>;
    async fn mark_used_memo(&self, id: i32) -> Result<()>;

    /// 出力ガードで弾いたメモを記録し、以後の取得対象から外す（同じメモを何度も試さないため）
    async fn mark_skipped_memo(&self, id: i32, reason: &str) -> Result<()>;

    /// 投稿したツイートを記録する。反響分析はこの tweet_id を起点に行う
    async fn record_posted_tweet(
        &self,
        tweet_id: &str,
        memo_id: i32,
        body: &str,
        tags: &str,
        source: Option<&str>,
    ) -> Result<()>;
}
