use axum::Form;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;

use crate::split::{SMS_LENGTH, split_into_messages};
use crate::{AppState, llm, twilio};

pub async fn index() -> &'static str {
    "askSMS is running"
}

/// Twilio's incoming-message webhook: answers the text with the LLM and replies by SMS.
pub async fn sms(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(params): Form<Vec<(String, String)>>,
) -> Result<impl IntoResponse, StatusCode> {
    let config = &state.config;

    let signature = headers
        .get("x-twilio-signature")
        .and_then(|v| v.to_str().ok());
    let valid = match (signature, twilio::request_url(&headers, &uri)) {
        (Some(signature), Some(url)) => {
            twilio::valid_signature(&config.twilio_auth_token, &url, &params, signature)
        }
        _ => false,
    };
    if !valid {
        tracing::warn!("rejecting request with an invalid Twilio signature");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let body = param(&params, "Body").unwrap_or_default();
    let from = param(&params, "From").ok_or(StatusCode::BAD_REQUEST)?;

    let answer = llm::get_answer(&state.llm, body).await.map_err(|err| {
        tracing::error!("getting an answer failed: {err:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let messages = split_into_messages(&answer.text, SMS_LENGTH);
    tracing::info!(cost = answer.cost, ?messages, "sending reply");

    twilio::send_messages(&state.http, config, from, &messages)
        .await
        .map_err(|err| {
            tracing::error!("sending messages failed: {err:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok((
        [(CONTENT_TYPE, "text/xml; charset=utf-8")],
        twilio::EMPTY_TWIML,
    ))
}

/// Returns a form param's value; as with Rack, the last one wins if a key repeats.
fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}
