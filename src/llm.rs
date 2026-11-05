use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Config;

const INSTRUCTIONS: &str = "Focus on being clear and direct. DO NOT use emojis, Markdown, or \
    citations. Use only raw plaintext compatible with GSM-7 encoding.";
const MAX_OUTPUT_TOKENS: u32 = 128;
const MAX_ANSWER_CHARS: usize = 800;
const CONCISE_PROMPT: &str = "Please make your answer more concise.";

/// The model's final answer and the total cost of the requests that produced it.
#[derive(Debug)]
pub struct Answer {
    pub text: String,
    pub cost: f64,
}

#[derive(Debug, Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

impl Message {
    fn new(role: &'static str, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    cost: Option<f64>,
}

/// Asks the model to answer the incoming message. If the first answer is too long,
/// asks it (in the same conversation) to make the answer more concise.
pub async fn get_answer(
    http: &reqwest::Client,
    config: &Config,
    incoming: &str,
) -> anyhow::Result<Answer> {
    let mut messages = vec![
        Message::new("system", INSTRUCTIONS),
        Message::new("user", incoming),
    ];

    let (mut text, mut cost) = complete(http, config, &messages).await?;

    let len = text.chars().count();
    if len > MAX_ANSWER_CHARS {
        tracing::info!(len, "response too long, asking for a more concise answer");
        messages.push(Message::new("assistant", text));
        messages.push(Message::new("user", CONCISE_PROMPT));
        let (concise, concise_cost) = complete(http, config, &messages).await?;
        text = concise;
        cost += concise_cost;
    }

    Ok(Answer { text, cost })
}

/// Sends one chat completion request and returns the reply's text and cost.
async fn complete(
    http: &reqwest::Client,
    config: &Config,
    messages: &[Message],
) -> anyhow::Result<(String, f64)> {
    let body = json!({
        "model": config.openrouter_model,
        "messages": messages,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "plugins": [{ "id": "web" }],
        "usage": { "include": true },
    });

    let res = http
        .post(format!("{}/chat/completions", config.openrouter_base_url))
        .bearer_auth(&config.openrouter_api_key)
        .json(&body)
        .send()
        .await?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        bail!("OpenRouter returned {status}: {text}");
    }

    let response: ChatResponse = res.json().await.context("parsing OpenRouter response")?;
    let usage = response.usage;
    let choice = response
        .choices
        .into_iter()
        .next()
        .context("OpenRouter response has no choices")?;
    let text = choice.message.content.unwrap_or_default();

    tracing::info!(
        finish_reason = choice.finish_reason.as_deref(),
        prompt_tokens = usage.as_ref().and_then(|u| u.prompt_tokens),
        completion_tokens = usage.as_ref().and_then(|u| u.completion_tokens),
        cost = usage.as_ref().and_then(|u| u.cost),
        text,
        "LLM response"
    );

    let cost = usage.and_then(|u| u.cost).unwrap_or(0.0);
    Ok((text, cost))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn completion(content: &str, cost: f64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 20, "cost": cost }
        }))
    }

    #[tokio::test]
    async fn gets_an_answer_from_the_llm() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-openrouter-key"))
            .and(body_partial_json(json!({
                "model": "test/model",
                "messages": [
                    { "role": "system", "content": INSTRUCTIONS },
                    { "role": "user", "content": "When does the brewery open?" }
                ],
                "max_tokens": 128,
                "plugins": [{ "id": "web" }]
            })))
            .respond_with(completion("It opens at 11:00 AM on Saturday.", 0.0012))
            .expect(1)
            .mount(&server)
            .await;

        let config = Config::for_tests(&server.uri(), &server.uri());
        let answer = get_answer(
            &reqwest::Client::new(),
            &config,
            "When does the brewery open?",
        )
        .await
        .unwrap();

        assert_eq!(answer.text, "It opens at 11:00 AM on Saturday.");
        assert_eq!(answer.cost, 0.0012);
    }

    #[tokio::test]
    async fn asks_for_a_concise_answer_when_too_long() {
        let server = MockServer::start().await;
        let long_answer = "word ".repeat(200);

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(json!({
                "messages": [
                    { "role": "system" },
                    { "role": "user", "content": "Tell me everything" },
                    { "role": "assistant", "content": long_answer },
                    { "role": "user", "content": CONCISE_PROMPT }
                ]
            })))
            .respond_with(completion("Short version.", 0.002))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(completion(&long_answer, 0.001))
            .expect(1)
            .mount(&server)
            .await;

        let config = Config::for_tests(&server.uri(), &server.uri());
        let answer = get_answer(&reqwest::Client::new(), &config, "Tell me everything")
            .await
            .unwrap();

        assert_eq!(answer.text, "Short version.");
        assert!((answer.cost - 0.003).abs() < 1e-12);

        let requests = server.received_requests().await.unwrap();
        let last: Value = requests.last().unwrap().body_json().unwrap();
        assert_eq!(last["messages"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn returns_an_error_when_openrouter_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let config = Config::for_tests(&server.uri(), &server.uri());
        let err = get_answer(&reqwest::Client::new(), &config, "hi")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
    }
}
