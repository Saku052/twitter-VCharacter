use anyhow::Result;
use async_trait::async_trait;
use crate::ports::ai_generator::AiGenerator;
const CONTENTS: &str = "今日もコード書いてた";
pub struct StaticContent;

impl StaticContent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AiGenerator for StaticContent {
    async fn generate(&self) -> Result<String> {
        Ok(CONTENTS.to_string())
    }
}
