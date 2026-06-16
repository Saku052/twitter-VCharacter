use async_trait::async_trait;
use serde::Deserialize;
use crate::ports::youtube_port::YoutubePort;
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
    
    async fn get_youtube_video(&self) -> Result<(String, String)>{
        let client = reqwest::Client::new();

        let response = client
            .get("https://www.googleapis.com/youtube/v3/playlistItems")
            .query(&[
                ("part", "snippet"),
                ("playlistId", "PLmnlM73lBYpMeZRdjxpTRvoTDKfJG-pIA"),
                ("maxResults", "30"),
                ("key", self.api_key.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?;

        // TODO: responseを噛み砕いたものをかく
        let body: YoutubeResponse = response.json().await?;
        let latest = body
            .items
            .into_iter()
            .last()
            .ok_or_else(|| anyhow::anyhow!("playlist is empty"))?;

        // TODO: return　としてsnippetを返すの一番理想的
        Ok((latest.snippet.title, latest.snippet.description))
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
    description: String
}