use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
pub const PHASE3_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

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
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

#[derive(Deserialize, Default, Debug)]
pub struct ChatUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    #[allow(dead_code)]
    pub total_tokens: Option<u32>,
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
    timeout: Option<Duration>,
) -> Result<String> {
    let request = ChatRequest {
        model: model.to_string(),
        messages,
        stream: false,
    };

    send_chat_request(&request, api_key, api_url, timeout).await
}

pub async fn send_chat_request(
    request: &ChatRequest,
    api_key: &str,
    api_url: &str,
    timeout: Option<Duration>,
) -> Result<String> {
    let max_retries = 3;
    let mut last_err = None;

    let effective_timeout = timeout.unwrap_or(REQUEST_TIMEOUT);
    let client = reqwest::Client::builder()
        .timeout(effective_timeout)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let msg_count = request.messages.len();
    let approx_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
    eprintln!("  📡 API request → model={}, msgs={}, ~{}K chars", request.model, msg_count, approx_chars / 1000);

    for attempt in 0..=max_retries {
        if attempt > 0 {
            // Exponential backoff with jitter: base 1s, 2s, 4s + random [0, 500ms)
            let base_ms = 1000u64 * (1u64 << (attempt - 1));
            let jitter_ms = rand_jitter();
            let delay_ms = base_ms + jitter_ms;
            eprintln!("  ⏳ Retry {attempt}/{max_retries} (waiting {}ms)...", delay_ms);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }

        let start = std::time::Instant::now();
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
            let elapsed = start.elapsed();
            let resp_len = chat_resp.choices.first().map(|c| c.message.content.len()).unwrap_or(0);
            let usage_str = match &chat_resp.usage {
                Some(u) => format!(
                    "tokens={}/{}",
                    u.prompt_tokens.unwrap_or(0),
                    u.completion_tokens.unwrap_or(0)
                ),
                None => "tokens=n/a".to_string(),
            };
            eprintln!(
                "  ✅ API response ← {}ms, ~{}K chars out, {}",
                elapsed.as_millis(),
                resp_len / 1000,
                usage_str
            );
            return Ok(chat_resp
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default());
        }

        // Retry on transient errors: 429 (rate limit), 5xx (server error)
        let elapsed = start.elapsed();
        let should_retry = status.as_u16() == 429 || status.is_server_error();
        let body = resp.text().await.unwrap_or_default();

        eprintln!(
            "  ❌ API error ← {} {} ({}ms, ~{}K chars body)",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            elapsed.as_millis(),
            body.chars().count() / 1000
        );

        if should_retry && attempt < max_retries {
            eprintln!("  ⚠ Retrying...");
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

/// Simple jitter: random [0, 500) ms using system time as entropy source.
fn rand_jitter() -> u64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 500) as u64
}
