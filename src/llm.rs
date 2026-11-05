use rig_core::Model;
use rig_core::completion::{CompletionRequest, Message};
use rig_core::providers::openai::wire::{OPENROUTER, OpenAiWire};
use rig_core::providers::openai::{OpenAIConfig, Route};
use rig_core::providers::openrouter::extension::{OpenRouterExt, OpenRouterOptions};
use rig_reqwest::ReqwestClient;
use serde_json::json;

use crate::Config;

const INSTRUCTIONS: &str = "Focus on being clear and direct. DO NOT use emojis, Markdown, or \
    citations. Use only raw plaintext compatible with GSM-7 encoding.";
const MAX_OUTPUT_TOKENS: u64 = 128;
const MAX_ANSWER_CHARS: usize = 800;
const CONCISE_PROMPT: &str = "Please make your answer more concise.";

/// The OpenRouter chat model every request goes to.
pub type LlmModel = Model<OpenAiWire>;

/// The model's final answer and the total cost of the requests that produced it.
#[derive(Debug)]
pub struct Answer {
    pub text: String,
    pub cost: f64,
}

/// Builds the OpenRouter model, sending through `http` so its timeout applies.
pub fn model(http: &reqwest::Client, config: &Config) -> LlmModel {
    OpenAIConfig::with_key(&OPENROUTER, config.openrouter_api_key.clone())
        .with_base_url(config.openrouter_base_url.clone())
        .with_route(Route::Chat)
        .connect(ReqwestClient::from(http.clone()))
        .completion(config.openrouter_model.clone())
}

/// Asks the model to answer the incoming message. If the first answer is too long,
/// asks it (in the same conversation) to make the answer more concise.
pub async fn get_answer(model: &LlmModel, incoming: &str) -> anyhow::Result<Answer> {
    let (mut text, mut cost) = ask(model, &[], incoming).await?;

    let len = text.chars().count();
    if len > MAX_ANSWER_CHARS {
        tracing::info!(len, "response too long, asking for a more concise answer");
        let history = [Message::user(incoming), Message::assistant(text)];
        let (concise, concise_cost) = ask(model, &history, CONCISE_PROMPT).await?;
        text = concise;
        cost += concise_cost;
    }

    Ok(Answer { text, cost })
}

/// Sends `prompt` after `history` and returns the reply's text and cost.
async fn ask(model: &LlmModel, history: &[Message], prompt: &str) -> anyhow::Result<(String, f64)> {
    let request = CompletionRequest::new(Message::user(prompt))
        .preamble(INSTRUCTIONS)
        .messages(history.iter().cloned())
        .max_tokens(MAX_OUTPUT_TOKENS)
        .provider_option(OpenRouterOptions::new().plugin(json!({ "id": "web" })));

    let response = model.call(request).await?;
    let text = response.text();
    let cost = response
        .extras::<OpenRouterExt>()
        .and_then(Result::ok)
        .and_then(|extras| extras.cost);
    let logs_url = response
        .response_id()
        .map(|id| format!("https://openrouter.ai/logs?transaction={id}"));

    tracing::info!(
        logs_url,
        finish_reason = ?response.finish_reason(),
        input_tokens = response.usage.input_tokens,
        output_tokens = response.usage.output_tokens,
        cost,
        text,
        "LLM response"
    );

    Ok((text, cost.unwrap_or(0.0)))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn completion(content: &str, cost: f64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "id": "gen-1",
            "object": "chat.completion",
            "created": 1_761_284_632,
            "model": "test/model",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30,
                "cost": cost
            }
        }))
    }

    fn test_model(server: &MockServer) -> LlmModel {
        model(
            &reqwest::Client::new(),
            &Config::for_tests(&server.uri(), &server.uri()),
        )
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
                    { "role": "system", "content": [{ "type": "text", "text": INSTRUCTIONS }] },
                    { "role": "user", "content": "When does the brewery open?" }
                ],
                "max_tokens": 128,
                "plugins": [{ "id": "web" }]
            })))
            .respond_with(completion("It opens at 11:00 AM on Saturday.", 0.0012))
            .expect(1)
            .mount(&server)
            .await;

        let answer = get_answer(&test_model(&server), "When does the brewery open?")
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
                    { "role": "assistant" },
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

        let answer = get_answer(&test_model(&server), "Tell me everything")
            .await
            .unwrap();

        assert_eq!(answer.text, "Short version.");
        assert!((answer.cost - 0.003).abs() < 1e-12);

        let requests = server.received_requests().await.unwrap();
        let last: Value = requests.last().unwrap().body_json().unwrap();
        assert_eq!(last["messages"].as_array().unwrap().len(), 4, "{last}");
    }

    #[tokio::test]
    async fn returns_an_error_when_openrouter_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let err = get_answer(&test_model(&server), "hi").await.unwrap_err();
        assert!(err.to_string().contains("401"), "{err:#}");
    }
}
