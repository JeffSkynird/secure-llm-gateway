use anyhow::Context;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio_stream::wrappers::LinesStream;
use tokio_util::io::StreamReader;

#[derive(Clone)]
pub struct OpenAIProvider {
    api_key: String,
    client: Client,
    base_url: Url,
}

impl OpenAIProvider {
    pub fn new(api_key: String, base_url: Option<String>) -> anyhow::Result<Self> {
        let base = match base_url {
            Some(u) if !u.is_empty() => Url::parse(&u)?,
            _ => Url::parse("https://api.openai.com")?,
        };
        let mut builder = Client::builder();
        if base.scheme() == "https" {
            builder = builder.http2_prior_knowledge();
        }
        let client = builder.build()?;
        Ok(Self {
            api_key,
            client,
            base_url: base,
        })
    }

    pub async fn chat_stream(
        &self,
        mut payload: ChatCompletionRequest,
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<String>>> {
        let url = self.base_url.join("/v1/chat/completions")?;
        // ensure streaming
        payload.stream = Some(true);

        let res = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .await
            .context("openai send failed")?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("openai error: {} - {}", status, body));
        }

        let stream = res
            .bytes_stream()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        // Transform into line-based "data: ..." items
        let s = StreamReader::new(stream);
        let reader = tokio::io::BufReader::new(s);
        let lines = reader.lines();
        let lines = LinesStream::new(lines).map(|l| l.map_err(|e| e.into()));
        Ok(lines)
    }

    pub async fn chat_completion(
        &self,
        mut payload: ChatCompletionRequest,
    ) -> anyhow::Result<OpenAIChatCompletionResponse> {
        let url = self.base_url.join("/v1/chat/completions")?;
        payload.stream = Some(false);

        let res = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .header("Accept", "application/json")
            .json(&payload)
            .send()
            .await
            .context("openai send failed")?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("openai error: {} - {}", status, body));
        }

        let body = res
            .json::<OpenAIChatCompletionResponse>()
            .await
            .context("failed to parse openai response")?;
        Ok(body)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIStreamChunk {
    pub id: Option<String>,
    pub choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIChoice {
    pub index: Option<u32>,
    pub delta: Option<OpenAIDelta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIDelta {
    pub role: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIChatCompletionResponse {
    pub id: Option<String>,
    pub object: Option<String>,
    pub created: Option<i64>,
    pub model: Option<String>,
    pub choices: Vec<OpenAIChatCompletionChoice>,
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIChatCompletionChoice {
    pub index: Option<u32>,
    pub message: Option<ChatMessage>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}
