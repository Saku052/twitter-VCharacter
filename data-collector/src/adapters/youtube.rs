use async_trait::async_trait;
use serde::Deserialize;
use crate::ports::youtube_port::youtube_port;
use anyhow::Result;

pub struct youtube_client {
    api_key: String
}

impl youtube_client {
    pub fn new(api_key: String) -> Self {
        Self {api_key}
    }
}

#[async_trait]
impl youtube_port for youtube_client {
    
    async fn get_youtube_video(&self) -> Result<(String, String)>{
        let client = reqwest::Client::new();

        let response = client
    .get("https://www.googleapis.com/youtube/v3/playlistItems")
    .query(&[
        ("part", "snippet"),
        ("playlistId", "PLmnlM73lBYpMeZRdjxpTRvoTDKfJG-pIA"),
        ("maxResults", "10"),
        ("key", &self.api_key.as_str()),
    ])
    .send()
    .await?;
        
        // TODO: responseを噛み砕いたものをかく
        let body: youtubeResponse = response.json().await?;
        let title = body.items[0].snippet.title.clone();
        let description = body.items[0].snippet.description.clone();

        // TODO: return　としてsnippetを返すの一番理想的
        Ok((title, description))
    }
    
}

//----------------------------------------------------------------------------------------------------------------------------
//Youtube JSON Response
//----------------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct youtubeResponse {
    items: Vec<youtubeItem>
}

#[derive(Deserialize)]
struct youtubeItem {
    snippet: Snippet
}

#[derive(Deserialize)]
struct Snippet {
    title: String,
    description: String
}