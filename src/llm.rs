use rig_core::Model;
use rig_core::completion::{CompletionRequest, Effort, FinishReason, Message};
use rig_core::providers::openai::wire::{OPENROUTER, OpenAiWire};
use rig_core::providers::openai::{OpenAIConfig, Route};
use rig_core::providers::openrouter::extension::{OpenRouterExt, OpenRouterOptions};
use rig_reqwest::ReqwestClient;
use serde_json::{Value, json};

pub const INSTRUCTIONS: &str = "Your reply is sent as a text message, so keep it short: one to \
    three sentences, under 300 characters, unless the question truly needs more. Be clear and \
    direct. DO NOT use emojis, Markdown, or citations. Use only raw plaintext compatible with \
    GSM-7 encoding.";
/// A safety limit on cost and latency, not a way to keep answers short: the model
/// doesn't see it, so it only cuts replies off. The prompt keeps answers short.
/// It's high because reasoning models count their reasoning against it. It isn't
/// removed because without it OpenRouter reserves the model's whole output
/// window against the key's spending limit.
pub const MAX_OUTPUT_TOKENS: u64 = 8000;
/// Replaces the web plugin's default search prompt, which asks for Markdown links
/// to sources.
pub const SEARCH_PROMPT: &str = "Use the following web search results to answer. Do not cite \
    them: no links, URLs, or lists of sources.";
pub const MAX_ANSWER_CHARS: usize = 800;
pub const CONCISE_PROMPT: &str = "Please make your answer more concise.";

/// The OpenRouter chat model every request goes to.
pub type LlmModel = Model<OpenAiWire>;

/// How the model is asked to answer: the system prompt, output cap, OpenRouter
/// plugins and reasoning effort. The default is what production uses.
#[derive(Clone, Debug, PartialEq)]
pub struct AnswerConfig {
    pub instructions: String,
    pub max_output_tokens: u64,
    pub plugins: Vec<Value>,
    pub reasoning: Option<Effort>,
}

impl Default for AnswerConfig {
    fn default() -> Self {
        Self {
            instructions: INSTRUCTIONS.to_owned(),
            max_output_tokens: MAX_OUTPUT_TOKENS,
            plugins: vec![json!({ "id": "web", "search_prompt": SEARCH_PROMPT })],
            reasoning: None,
        }
    }
}

/// The model's final answer and the total cost of the requests that produced it.
#[derive(Debug)]
pub struct Answer {
    pub text: String,
    pub cost: f64,
    /// Whether the first answer was too long, so the model was asked to shorten it.
    pub asked_for_concise: bool,
    /// Whether the final reply was cut off by the output-token limit.
    pub truncated: bool,
}

/// One reply's text and cost.
#[derive(Debug)]
pub struct Reply {
    pub text: String,
    pub cost: f64,
    /// Whether the output-token limit cut the reply off.
    pub truncated: bool,
}

/// Builds an OpenRouter model, sending through `http` so its timeout applies.
pub fn model(http: &reqwest::Client, base_url: &str, api_key: &str, model_id: &str) -> LlmModel {
    OpenAIConfig::with_key(&OPENROUTER, api_key.to_owned())
        .with_base_url(base_url.to_owned())
        .with_route(Route::Chat)
        .connect(ReqwestClient::from(http.clone()))
        .completion(model_id.to_owned())
}

/// Asks the model to answer the incoming message. If the first answer is too long,
/// asks it (in the same conversation) to make the answer more concise. That
/// follow-up goes without plugins, so the web plugin doesn't search for the
/// follow-up's own text.
pub async fn get_answer(
    model: &LlmModel,
    config: &AnswerConfig,
    incoming: &str,
) -> anyhow::Result<Answer> {
    let first = ask(model, config, &[], incoming).await?;
    let mut text = first.text;
    let mut cost = first.cost;
    let mut truncated = first.truncated;
    let mut asked_for_concise = false;

    let len = text.chars().count();
    if len > MAX_ANSWER_CHARS {
        tracing::info!(len, "response too long, asking for a more concise answer");
        let history = [Message::user(incoming), Message::assistant(text)];
        let without_plugins = AnswerConfig {
            plugins: Vec::new(),
            ..config.clone()
        };
        let concise = ask(model, &without_plugins, &history, CONCISE_PROMPT).await?;
        text = concise.text;
        cost += concise.cost;
        truncated = concise.truncated;
        asked_for_concise = true;
    }

    Ok(Answer {
        text,
        cost,
        asked_for_concise,
        truncated,
    })
}

/// Sends `prompt` after `history` with the settings in `config`.
async fn ask(
    model: &LlmModel,
    config: &AnswerConfig,
    history: &[Message],
    prompt: &str,
) -> anyhow::Result<Reply> {
    let mut options = OpenRouterOptions::new();
    for plugin in &config.plugins {
        options = options.plugin(plugin.clone());
    }
    let mut request = CompletionRequest::new(Message::user(prompt))
        .preamble(config.instructions.clone())
        .messages(history.iter().cloned())
        .max_tokens(config.max_output_tokens)
        .provider_option(options);
    if let Some(effort) = config.reasoning {
        request = request.reasoning(effort);
    }
    send(model, request).await
}

/// Sends `request` and returns the reply's text and its cost as OpenRouter reports it.
pub async fn send(model: &LlmModel, request: CompletionRequest) -> anyhow::Result<Reply> {
    let response = model.call(request).await?;
    let text = response.text();
    let finish_reason = response.finish_reason();
    let cost = response
        .extras::<OpenRouterExt>()
        .and_then(Result::ok)
        .and_then(|extras| extras.cost);
    let logs_url = response
        .response_id()
        .map(|id| format!("https://openrouter.ai/logs?transaction={id}"));

    tracing::info!(
        logs_url,
        ?finish_reason,
        input_tokens = response.usage.input_tokens,
        output_tokens = response.usage.output_tokens,
        cost,
        text,
        "LLM response"
    );

    Ok(Reply {
        text,
        cost: cost.unwrap_or(0.0),
        truncated: finish_reason == Some(FinishReason::Length),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn completion(content: &str, cost: f64) -> ResponseTemplate {
        completion_with(content, cost, "stop")
    }

    fn completion_with(content: &str, cost: f64, finish_reason: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "id": "gen-1",
            "object": "chat.completion",
            "created": 1_761_284_632,
            "model": "test/model",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": finish_reason
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
            &server.uri(),
            "test-openrouter-key",
            "test/model",
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
                "max_tokens": MAX_OUTPUT_TOKENS,
                "plugins": AnswerConfig::default().plugins
            })))
            .respond_with(completion("It opens at 11:00 AM on Saturday.", 0.0012))
            .expect(1)
            .mount(&server)
            .await;

        let answer = get_answer(
            &test_model(&server),
            &AnswerConfig::default(),
            "When does the brewery open?",
        )
        .await
        .unwrap();

        assert_eq!(answer.text, "It opens at 11:00 AM on Saturday.");
        assert_eq!(answer.cost, 0.0012);
        assert!(!answer.truncated);
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

        let answer = get_answer(
            &test_model(&server),
            &AnswerConfig::default(),
            "Tell me everything",
        )
        .await
        .unwrap();

        assert_eq!(answer.text, "Short version.");
        assert!((answer.cost - 0.003).abs() < 1e-12);

        let requests = server.received_requests().await.unwrap();
        let first: Value = requests[0].body_json().unwrap();
        let last: Value = requests.last().unwrap().body_json().unwrap();
        assert_eq!(last["messages"].as_array().unwrap().len(), 4, "{last}");
        assert_eq!(first["plugins"], json!(AnswerConfig::default().plugins));
        assert_eq!(last.get("plugins"), None, "{last}");
    }

    #[tokio::test]
    async fn sends_the_answer_config() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(completion("Hi.", 0.0))
            .expect(1)
            .mount(&server)
            .await;

        let config = AnswerConfig {
            instructions: "Be brief.".to_owned(),
            max_output_tokens: 300,
            plugins: vec![json!({ "id": "web", "max_results": 2 })],
            reasoning: Some(Effort::Low),
        };
        get_answer(&test_model(&server), &config, "hi")
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        assert_eq!(body["messages"][0]["content"][0]["text"], "Be brief.");
        assert_eq!(body["max_tokens"], 300);
        assert_eq!(body["plugins"], json!([{ "id": "web", "max_results": 2 }]));
        assert_eq!(body["reasoning"], json!({ "effort": "low" }));
    }

    #[tokio::test]
    async fn reports_a_reply_cut_off_by_the_token_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(completion_with("It opens at", 0.001, "length"))
            .mount(&server)
            .await;

        let answer = get_answer(&test_model(&server), &AnswerConfig::default(), "hi")
            .await
            .unwrap();
        assert!(answer.truncated);
    }

    #[tokio::test]
    async fn returns_an_error_when_openrouter_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let err = get_answer(&test_model(&server), &AnswerConfig::default(), "hi")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"), "{err:#}");
    }
}
