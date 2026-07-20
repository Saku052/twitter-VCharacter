use async_trait::async_trait;
use serde::Deserialize;
use crate::ports::youtube_port::YoutubePort;
use crate::domain::VideoInfo;
use anyhow::Result;

const DEFAULT_BASE_URL: &str = "https://www.googleapis.com/youtube/v3";

pub struct YoutubeClient {
    api_key: String,
    base_url: String,
}

impl YoutubeClient {
    pub fn new(api_key: String) -> Self {
        Self { api_key, base_url: DEFAULT_BASE_URL.to_string() }
    }

    #[cfg(test)]
    fn with_base_url(api_key: String, base_url: String) -> Self {
        Self { api_key, base_url }
    }
}

#[async_trait]
impl YoutubePort for YoutubeClient {

    async fn fetch_recent_videos(&self) -> Result<Vec<VideoInfo>>{
        let client = reqwest::Client::new();

        let response = client
            .get(format!("{}/playlistItems", self.base_url))
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    #[tokio::test]
    async fn returns_5_most_recent_videos_in_newest_first_order() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "items": [
                { "snippet": { "title": "old-1", "description": "", "resourceId": { "videoId": "old-1" } } },
                { "snippet": { "title": "old-2", "description": "", "resourceId": { "videoId": "old-2" } } },
                { "snippet": { "title": "mid-1", "description": "", "resourceId": { "videoId": "mid-1" } } },
                { "snippet": { "title": "mid-2", "description": "", "resourceId": { "videoId": "mid-2" } } },
                { "snippet": { "title": "new-1", "description": "", "resourceId": { "videoId": "new-1" } } },
                { "snippet": { "title": "new-2", "description": "", "resourceId": { "videoId": "new-2" } } }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/playlistItems"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = YoutubeClient::with_base_url("dummy-key".to_string(), mock_server.uri());
        let videos = client.fetch_recent_videos().await.unwrap();

        assert_eq!(videos.len(), 5);
        assert_eq!(videos[0].video_id, "new-2");
        assert_eq!(videos[4].video_id, "old-2");
        assert!(videos.iter().all(|v| v.video_id != "old-1"));
    }
}
