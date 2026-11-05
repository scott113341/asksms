mod llm;
mod routes;
mod split;
mod twilio;

use std::env;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};

pub struct Config {
    pub listen: String,
    pub openrouter_api_key: String,
    pub openrouter_model: String,
    pub openrouter_base_url: String,
    pub twilio_account_sid: String,
    pub twilio_auth_token: String,
    pub twilio_number: String,
    pub twilio_base_url: String,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            listen: env_or("LISTEN", "0.0.0.0:4567"),
            openrouter_api_key: required("OPENROUTER_API_KEY")?,
            openrouter_model: env_or("OPENROUTER_MODEL", "openai/gpt-5-nano"),
            openrouter_base_url: env_or("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1"),
            twilio_account_sid: required("TWILIO_ACCOUNT_SID")?,
            twilio_auth_token: required("TWILIO_AUTH_TOKEN")?,
            twilio_number: required("TWILIO_NUMBER")?,
            twilio_base_url: env_or("TWILIO_BASE_URL", "https://api.twilio.com"),
        })
    }

    #[cfg(test)]
    fn for_tests(openrouter_base_url: &str, twilio_base_url: &str) -> Self {
        Self {
            listen: "127.0.0.1:0".to_owned(),
            openrouter_api_key: "test-openrouter-key".to_owned(),
            openrouter_model: "test/model".to_owned(),
            openrouter_base_url: openrouter_base_url.to_owned(),
            twilio_account_sid: "ACtest".to_owned(),
            twilio_auth_token: "test-auth-token".to_owned(),
            twilio_number: "+15550001111".to_owned(),
            twilio_base_url: twilio_base_url.to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::index))
        .route("/sms", post(routes::sms))
        .with_state(state)
}

fn env_or(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{name} must be set"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    tracing::info!("listening on {}", listener.local_addr()?);

    let state = AppState {
        config: Arc::new(config),
        http,
    };
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await.ok();
}

/// End-to-end tests through the router, with OpenRouter and Twilio mocked by wiremock.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const ANSWER: &str = "MainStage Brewing in Lyons opens at 11:00 AM on Saturday and \
        stays open until 9:00 PM. Some local listings show an opening time of noon, but the \
        brewery's own website lists 11:00 AM. Hours can change for special events and live \
        music nights, so it is a good idea to check their website or social media before \
        you go. The taproom is on Main Street and is dog friendly.";
    const FROM: &str = "+11231231234";

    struct Mocks {
        openrouter: MockServer,
        twilio: MockServer,
    }

    impl Mocks {
        async fn start() -> Self {
            Self {
                openrouter: MockServer::start().await,
                twilio: MockServer::start().await,
            }
        }

        fn app(&self) -> Router {
            app(AppState {
                config: Arc::new(Config::for_tests(
                    &self.openrouter.uri(),
                    &self.twilio.uri(),
                )),
                http: reqwest::Client::new(),
            })
        }
    }

    fn form(params: &[(&str, &str)]) -> Vec<(String, String)> {
        params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn sms_request(params: &[(String, String)], signature: &str) -> Request<Body> {
        Request::post("/sms")
            .header("host", "asksms.example.com")
            .header("x-forwarded-proto", "https")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-twilio-signature", signature)
            .body(Body::from(serde_urlencoded::to_string(params).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn index_responds() {
        let mocks = Mocks::start().await;
        let res = mocks
            .app()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, "askSMS is running");
    }

    #[tokio::test]
    async fn sms_answers_and_replies() {
        let mocks = Mocks::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": ANSWER }, "finish_reason": "stop" }],
                "usage": { "cost": 0.0061 }
            })))
            .expect(1)
            .mount(&mocks.openrouter)
            .await;
        Mock::given(method("POST"))
            .and(path("/2010-04-01/Accounts/ACtest/Messages.json"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sid": "SM1" })))
            .expect(3)
            .mount(&mocks.twilio)
            .await;

        let params = form(&[
            ("Body", "Who is the current president?"),
            ("From", FROM),
            ("To", "+15550001111"),
        ]);
        let signature = twilio::sign("test-auth-token", "https://asksms.example.com/sms", &params);
        let res = mocks
            .app()
            .oneshot(sms_request(&params, &signature))
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "text/xml; charset=utf-8");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, twilio::EMPTY_TWIML);

        let sent = mocks.twilio.received_requests().await.unwrap();
        assert_eq!(sent.len(), 3);
        for (i, request) in sent.iter().enumerate() {
            let fields: HashMap<String, String> =
                serde_urlencoded::from_bytes(&request.body).unwrap();
            assert_eq!(fields["To"], FROM);
            assert_eq!(fields["From"], "+15550001111");
            assert_eq!(fields["SmartEncoded"], "true");
            assert!(fields["Body"].starts_with(&format!("{}/3: ", i + 1)));
            assert!(fields["Body"].chars().count() <= 160);
        }
    }

    #[tokio::test]
    async fn sms_rejects_an_invalid_signature() {
        let mocks = Mocks::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mocks.openrouter)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mocks.twilio)
            .await;

        let params = form(&[("Body", "hi"), ("From", FROM)]);
        let signature = twilio::sign("wrong-token", "https://asksms.example.com/sms", &params);
        let res = mocks
            .app()
            .oneshot(sms_request(&params, &signature))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let unsigned = Request::post("/sms")
            .header("host", "asksms.example.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("Body=hi&From=%2B11231231234"))
            .unwrap();
        let res = mocks.app().oneshot(unsigned).await.unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
