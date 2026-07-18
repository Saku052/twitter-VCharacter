use async_trait::async_trait;
use serde::Deserialize;
use crate::ports::youtube_port::YoutubePort;
use crate::domain::VideoInfo;
use anyhow::Result;

pub struct YoutubeClient {
    api_key: String
}

impl YoutubeClient {
    pub fn new(api_key: String) -> Self {
        Self {api_key}
    }
}

#[async_trait]
impl YoutubePort for YoutubeClient {

    async fn fetch_recent_videos(&self) -> Result<Vec<VideoInfo>>{
        let client = reqwest::Client::new();

        let response = client
            .get("https://www.googleapis.com/youtube/v3/playlistItems")
            .query(&[
                ("part", "snippet"),
                ("playlistId", "PLmnlM73lBYpMeZRdjxpTRvoTDKfJG-pIA"),
                ("maxResults", "50"),
                ("key", self.api_key.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?;

        // YouTube playlistItems APIはプレイリストへの追加順（古い順）で返すため、
        // 新しい順に並べ替えてから直近5件を取る
        let body: YoutubeResponse = response.json().await?;
        let videos = body
            .items
            .into_iter()
            .rev()
            .take(5)
            .map(|item| VideoInfo {
                video_id: item.snippet.resource_id.video_id,
                title: item.snippet.title,
                description: item.snippet.description,
            })
            .collect();

        Ok(videos)
    }

}

//----------------------------------------------------------------------------------------------------------------------------
//Youtube JSON Response
//----------------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct YoutubeResponse {
    items: Vec<YoutubeItem>
}

#[derive(Deserialize)]
struct YoutubeItem {
    snippet: Snippet
}

#[derive(Deserialize)]
struct Snippet {
    title: String,
    description: String,
    #[serde(rename = "resourceId")]
    resource_id: ResourceId,
}

#[derive(Deserialize)]
struct ResourceId {
    #[serde(rename = "videoId")]
    video_id: String,
}
