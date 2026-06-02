use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
pub struct ChatChoice {
    pub message: ChatMessageContent,
}

#[derive(Deserialize)]
pub struct ChatMessageContent {
    pub content: String,
}

pub async fn chat_fresh(
    messages: Vec<ChatMessage>,
    api_key: &str,
    model: &str,
    api_url: &str,
) -> Result<String> {
    let request = ChatRequest {
        model: model.to_string(),
        messages,
        stream: false,
    };

    send_chat_request(&request, api_key, api_url).await
}

pub async fn send_chat_request(
    request: &ChatRequest,
    api_key: &str,
    api_url: &str,
) -> Result<String> {
    let max_retries = 3;
    let mut last_err = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            let delay_ms = 1000 * (1u64 << (attempt - 1)); // 1s, 2s, 4s
            eprintln!("  ⏳ 重试 {attempt}/{max_retries} (等待 {}ms)...", delay_ms);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }

        let client = reqwest::Client::new();
        let result = client
            .post(api_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(request)
            .send()
            .await;

        let resp = match result {
            Ok(r) => r,
            Err(source) => {
                last_err = Some(Error::ApiRequest {
                    context: "AI API request failed".to_string(),
                    source,
                });
                continue;
            }
        };

        let status = resp.status();
        if status.is_success() {
            let chat_resp: ChatResponse = resp
                .json()
                .await
                .map_err(|source| Error::ApiResponseDecode {
                    context: "Failed to parse AI response".to_string(),
                    source,
                })?;
            return Ok(chat_resp
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default());
        }

        // Retry on transient errors: 429 (rate limit), 5xx (server error)
        let should_retry = status.as_u16() == 429 || status.is_server_error();
        let body = resp.text().await.unwrap_or_default();

        if should_retry && attempt < max_retries {
            eprintln!("  ⚠ API 返回 {status}，将重试...");
            last_err = Some(Error::ApiResponse { status, body, url: api_url.to_string() });
            continue;
        }

        return Err(Error::ApiResponse { status, body, url: api_url.to_string() });
    }

    Err(last_err.unwrap_or_else(|| Error::ApiResponse {
        status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        body: "all retries exhausted".to_string(),
        url: api_url.to_string(),
    }))
}
