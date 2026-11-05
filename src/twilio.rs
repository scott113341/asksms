use anyhow::bail;
use axum::http::{HeaderMap, Uri};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

use crate::Config;

/// The empty TwiML document returned to Twilio's webhook.
pub const EMPTY_TWIML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Response></Response>\n";

/// Rebuilds the public URL Twilio sent the request to, as Rack's `request.url` does:
/// the scheme from `X-Forwarded-Proto` and the host from `X-Forwarded-Host` or `Host`.
pub fn request_url(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

    let scheme = header("x-forwarded-proto")
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("http");
    let host = header("x-forwarded-host")
        .and_then(|v| v.rsplit(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or_else(|| header("host"))?;
    let path = uri.path_and_query().map_or("/", |p| p.as_str());

    Some(format!("{scheme}://{host}{path}"))
}

/// Checks an `X-Twilio-Signature` header against the request URL and POST params.
/// Like twilio-ruby's `RequestValidator`, the URL is accepted with or without its port.
pub fn valid_signature(
    auth_token: &str,
    url: &str,
    params: &[(String, String)],
    signature: &str,
) -> bool {
    if auth_token.is_empty() {
        return false;
    }
    let Ok(signature) = BASE64.decode(signature) else {
        return false;
    };

    let mut params = params.to_vec();
    params.sort();

    url_variants(url).iter().any(|url| {
        signature_mac(auth_token, url, &params)
            .verify_slice(&signature)
            .is_ok()
    })
}

/// Computes the base64 signature Twilio would send for this URL and set of params.
#[cfg(test)]
pub fn sign(auth_token: &str, url: &str, params: &[(String, String)]) -> String {
    let mut params = params.to_vec();
    params.sort();
    BASE64.encode(
        signature_mac(auth_token, url, &params)
            .finalize()
            .into_bytes(),
    )
}

/// HMAC-SHA1 over the URL followed by each sorted param's key and value.
fn signature_mac(auth_token: &str, url: &str, sorted_params: &[(String, String)]) -> Hmac<Sha1> {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(auth_token.as_bytes()).expect("HMAC accepts any key length");
    mac.update(url.as_bytes());
    for (key, value) in sorted_params {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    mac
}

/// Returns the URL without a port and, when a port is known, with it (explicit or the
/// scheme's default).
fn url_variants(url: &str) -> Vec<String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return vec![url.to_owned()];
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            (host, Some(port))
        }
        _ => (authority, None),
    };
    let port = port.or(match scheme {
        "https" => Some("443"),
        "http" => Some("80"),
        _ => None,
    });

    let mut variants = vec![format!("{scheme}://{host}{tail}")];
    if let Some(port) = port {
        variants.push(format!("{scheme}://{host}:{port}{tail}"));
    }
    variants
}

/// Sends each message as its own SMS from the configured Twilio number.
pub async fn send_messages(
    http: &reqwest::Client,
    config: &Config,
    to: &str,
    messages: &[String],
) -> anyhow::Result<()> {
    let url = format!(
        "{}/2010-04-01/Accounts/{}/Messages.json",
        config.twilio_base_url, config.twilio_account_sid
    );

    for body in messages {
        let res = http
            .post(&url)
            .basic_auth(&config.twilio_account_sid, Some(&config.twilio_auth_token))
            .form(&[
                ("From", config.twilio_number.as_str()),
                ("To", to),
                ("Body", body.as_str()),
                ("SmartEncoded", "true"),
            ])
            .send()
            .await?;

        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            bail!("Twilio returned {status}: {text}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "12345";
    const URL: &str = "https://mycompany.com/myapp.php?foo=1&bar=2";
    // Example from Twilio's webhook security documentation.
    const SIGNATURE: &str = "0/KCTR6DLpKmkAf8muzZqo1nDgQ=";

    fn params() -> Vec<(String, String)> {
        [
            ("CallSid", "CA1234567890ABCDE"),
            ("Caller", "+12349013030"),
            ("Digits", "1234"),
            ("From", "+12349013030"),
            ("To", "+18005551212"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    #[test]
    fn accepts_a_valid_signature() {
        assert!(valid_signature(TOKEN, URL, &params(), SIGNATURE));
    }

    #[test]
    fn param_order_does_not_matter() {
        let mut params = params();
        params.reverse();
        assert!(valid_signature(TOKEN, URL, &params, SIGNATURE));
    }

    #[test]
    fn rejects_a_tampered_param() {
        let mut params = params();
        params[2].1 = "9999".to_owned();
        assert!(!valid_signature(TOKEN, URL, &params, SIGNATURE));
    }

    #[test]
    fn rejects_the_wrong_token_or_a_malformed_signature() {
        assert!(!valid_signature("54321", URL, &params(), SIGNATURE));
        assert!(!valid_signature(TOKEN, URL, &params(), "not base64!"));
        assert!(!valid_signature("", URL, &params(), SIGNATURE));
    }

    #[test]
    fn accepts_the_url_with_or_without_its_port() {
        let with_port = "https://mycompany.com:443/myapp.php?foo=1&bar=2";
        assert!(valid_signature(TOKEN, with_port, &params(), SIGNATURE));

        let signed_with_port = sign(TOKEN, with_port, &params());
        assert!(valid_signature(TOKEN, URL, &params(), &signed_with_port));
    }

    #[test]
    fn sign_matches_the_documented_example() {
        assert_eq!(sign(TOKEN, URL, &params()), SIGNATURE);
    }

    #[test]
    fn rebuilds_the_request_url_from_proxy_headers() {
        let uri: Uri = "/sms?x=1".parse().unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("host", "internal:4567".parse().unwrap());
        assert_eq!(
            request_url(&headers, &uri).as_deref(),
            Some("http://internal:4567/sms?x=1")
        );

        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        headers.insert("x-forwarded-host", "asksms.example.com".parse().unwrap());
        assert_eq!(
            request_url(&headers, &uri).as_deref(),
            Some("https://asksms.example.com/sms?x=1")
        );

        assert_eq!(request_url(&HeaderMap::new(), &uri), None);
    }
}
