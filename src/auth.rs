use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use urlencoding::{decode, encode};

pub const AUTH0_CLIENT_ID: &str = "26DJQIiBvq9Y4Ac3VZ0qeAbjdvYa9LEo";
pub const AUTH0_DOMAIN: &str = "polyhavenadmin.eu.auth0.com";
const AUTH0_SCOPE: &str = "openid profile email offline_access";
const AUTH_CALLBACK_BIND_ADDR: &str = "127.0.0.1:45873";
const AUTH_CALLBACK_URL: &str = "http://127.0.0.1:45873/callback";
const AUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedInIdentity {
    pub name: String,
    pub user_id: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserLogin {
    pub auth_url: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    pub login: BrowserLogin,
    code_verifier: String,
    state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    token_type: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

pub fn authorization_url() -> String {
    format!("https://{AUTH0_DOMAIN}/authorize")
}

pub fn token_url() -> String {
    format!("https://{AUTH0_DOMAIN}/oauth/token")
}

pub fn callback_url() -> &'static str {
    AUTH_CALLBACK_URL
}

pub fn auth0_audience() -> &'static str {
    "https://admin.polyhaven.com/api/phase"
}

pub fn userinfo_url() -> String {
    format!("https://{AUTH0_DOMAIN}/userinfo")
}

pub fn phase_api_base_url() -> &'static str {
    #[cfg(debug_assertions)]
    {
        "http://localhost:3001/"
    }
    #[cfg(not(debug_assertions))]
    {
        "https://admin.polyhaven.com/"
    }
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn start_pkce_authorization() -> AuthorizationRequest {
    let code_verifier = random_url_safe_string(64);
    let state = random_url_safe_string(32);
    let auth_url = build_authorization_url(&code_verifier, &state, callback_url());
    AuthorizationRequest {
        login: BrowserLogin {
            auth_url,
            redirect_uri: callback_url().to_string(),
        },
        code_verifier,
        state,
    }
}

pub fn login_with_pkce<F>(on_login_ready: F) -> Result<AuthTokens>
where
    F: FnOnce(&BrowserLogin),
{
    let listener = TcpListener::bind(AUTH_CALLBACK_BIND_ADDR).with_context(|| {
        format!("starting localhost login callback listener at {AUTH_CALLBACK_BIND_ADDR}")
    })?;
    listener
        .set_nonblocking(true)
        .context("configuring localhost login callback listener")?;

    let request = start_pkce_authorization();
    on_login_ready(&request.login);

    let code = wait_for_authorization_code(&listener, &request.state, AUTH_CALLBACK_TIMEOUT)?;
    exchange_authorization_code(&code, &request.code_verifier, callback_url())
}

pub fn exchange_authorization_code(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<AuthTokens> {
    let client = client()?;
    let resp = client
        .post(token_url())
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", AUTH0_CLIENT_ID),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier),
        ])
        .send()
        .context("exchanging Auth0 authorization code")?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Authentication required: Auth0 authorization code exchange failed {status}: {text}"
        ));
    }

    let token: TokenResponse =
        serde_json::from_str(&text).context("parsing Auth0 authorization token")?;
    Ok(tokens_from_response(token, now_unix_seconds())?)
}

pub fn refresh_access_token(refresh_token: &str) -> Result<AuthTokens> {
    let client = client()?;
    let resp = client
        .post(token_url())
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", AUTH0_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .context("refreshing Auth0 access token")?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Authentication required: token refresh failed {status}: {text}"
        ));
    }

    let token: TokenResponse =
        serde_json::from_str(&text).context("parsing Auth0 refresh token")?;
    let mut tokens = tokens_from_response(token, now_unix_seconds())?;
    if tokens.refresh_token.is_empty() {
        tokens.refresh_token = refresh_token.to_string();
    }
    Ok(tokens)
}

pub fn ensure_access_token(config: &mut crate::config::Config) -> Result<String> {
    if config.has_access_token() && !config.access_token_expired_at(now_unix_seconds()) {
        return Ok(config.auth_access_token.clone());
    }
    if !config.can_refresh_access_token() {
        return Err(anyhow!("Authentication required: please log in"));
    }

    let tokens = refresh_access_token(&config.auth_refresh_token)?;
    apply_tokens(config, &tokens);
    Ok(config.auth_access_token.clone())
}

pub fn apply_tokens(config: &mut crate::config::Config, tokens: &AuthTokens) {
    config.auth_access_token = tokens.access_token.clone();
    config.auth_refresh_token = tokens.refresh_token.clone();
    config.auth_expires_at = tokens.expires_at;
}

pub fn logged_in_identity(access_token: &str) -> Option<LoggedInIdentity> {
    let claims = decode_jwt_claims(access_token)?;
    let user_id = claim_string(&claims, &["sub"]).unwrap_or_else(|| "Unknown".to_string());
    let name = claim_string(
        &claims,
        &[
            "name",
            "preferred_username",
            "nickname",
            "email",
            "given_name",
        ],
    )
    .map(|value| {
        if value.contains('@') {
            value.split('@').next().unwrap_or(&value).to_string()
        } else {
            value
        }
    })
    .unwrap_or_else(|| user_id.clone());
    let role = claim_string(
        &claims,
        &[
            "role",
            "roles",
            "https://admin.polyhaven.com/role",
            "https://admin.polyhaven.com/roles",
            "https://polyhaven.com/role",
            "https://polyhaven.com/roles",
        ],
    )
    .unwrap_or_else(|| "Unknown".to_string());
    Some(LoggedInIdentity {
        name,
        user_id,
        role,
    })
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    sub: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    nickname: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

pub fn fetch_logged_in_identity(access_token: &str) -> Result<LoggedInIdentity> {
    let client = client()?;
    let resp = client
        .get(userinfo_url())
        .bearer_auth(access_token)
        .send()
        .context("fetching Auth0 userinfo")?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "Authentication required: Auth0 userinfo request failed {status}: {text}"
        ));
    }

    let userinfo: UserInfoResponse =
        serde_json::from_str(&text).context("parsing Auth0 userinfo response")?;
    let claims = decode_jwt_claims(access_token);
    let name = first_non_empty_text(&[
        userinfo.name.as_deref(),
        userinfo.preferred_username.as_deref(),
        userinfo.nickname.as_deref(),
        userinfo.email.as_deref(),
    ])
    .map(|value| {
        if value.contains('@') {
            value.split('@').next().unwrap_or(&value).to_string()
        } else {
            value
        }
    })
    .unwrap_or_else(|| userinfo.sub.clone());
    let role = claims
        .as_ref()
        .and_then(|claims| {
            claim_string(
                claims,
                &[
                    "role",
                    "roles",
                    "https://admin.polyhaven.com/role",
                    "https://admin.polyhaven.com/roles",
                    "https://polyhaven.com/role",
                    "https://polyhaven.com/roles",
                ],
            )
        })
        .unwrap_or_else(|| "Unknown".to_string());
    Ok(LoggedInIdentity {
        name,
        user_id: userinfo.sub,
        role,
    })
}

pub fn is_auth_required_error(message: &str) -> bool {
    message.starts_with("Authentication required")
}

pub(crate) fn tokens_from_response(token: TokenResponse, now: u64) -> Result<AuthTokens> {
    if let Some(error) = token.error {
        return Err(anyhow!("{}", token.error_description.unwrap_or(error)));
    }
    if token.access_token.trim().is_empty() {
        return Err(anyhow!("Auth0 response did not include an access token"));
    }
    Ok(AuthTokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token.unwrap_or_default(),
        expires_at: token.expires_in.map(|expires_in| now + expires_in),
    })
}

fn build_authorization_url(code_verifier: &str, state: &str, redirect_uri: &str) -> String {
    let challenge = pkce_challenge(code_verifier);
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&audience={}&code_challenge={}&code_challenge_method=S256&state={}",
        authorization_url(),
        encode(AUTH0_CLIENT_ID),
        encode(redirect_uri),
        encode(AUTH0_SCOPE),
        encode(auth0_audience()),
        encode(&challenge),
        encode(state),
    )
}

fn pkce_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn random_url_safe_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn wait_for_authorization_code(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<String> {
    let started_at = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                return handle_callback_stream(&mut stream, expected_state);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if started_at.elapsed() >= timeout {
                    bail!("Authentication required: browser login timed out");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(err).context("accepting localhost login callback"),
        }
    }
}

fn handle_callback_stream(stream: &mut TcpStream, expected_state: &str) -> Result<String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("configuring localhost callback read timeout")?;
    let mut buffer = [0_u8; 4096];
    let len = stream
        .read(&mut buffer)
        .context("reading localhost login callback")?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let result = parse_callback_request(&request, expected_state);
    let response: String = match &result {
        Ok(_) => success_http_response().to_string(),
        Err(err) => error_http_response(&err.to_string()),
    };
    stream
        .write_all(response.as_bytes())
        .context("writing localhost login callback response")?;
    result
}

fn parse_callback_request(request: &str, expected_state: &str) -> Result<String> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow!("Authentication required: empty login callback"))?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        bail!("Authentication required: invalid login callback method");
    }

    let (path, query) = target
        .split_once('?')
        .ok_or_else(|| anyhow!("Authentication required: login callback missing query"))?;
    if path != "/callback" {
        bail!("Authentication required: invalid login callback path");
    }

    let params = parse_query(query)?;
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        bail!("Authentication required: Auth0 login failed: {description}");
    }
    let state = params
        .get("state")
        .ok_or_else(|| anyhow!("Authentication required: login callback missing state"))?;
    if state != expected_state {
        bail!("Authentication required: login state mismatch");
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| anyhow!("Authentication required: login callback missing code"))
}

fn parse_query(query: &str) -> Result<std::collections::HashMap<String, String>> {
    let mut params = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(
            decode(key)
                .context("decoding login callback query key")?
                .into_owned(),
            decode(value)
                .context("decoding login callback query value")?
                .into_owned(),
        );
    }
    Ok(params)
}

fn success_http_response() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>PHASE login complete</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#1a1a1a;color:#eee;font:16px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}main{max-width:30rem;padding:2rem 2.25rem;border:1px solid rgba(238,238,238,.12);border-radius:16px;background:rgba(26,26,26,.96);box-shadow:0 18px 48px rgba(0,0,0,.35)}h1{margin:0 0 .5rem;color:#e14d5b;font-size:1.4rem}p{margin:0;color:#eee}</style></head><body><main><h1>PHASE login complete</h1><p>You can close this tab and return to <strong>PHASE</strong>.</p></main></body></html>"
}

fn error_http_response(message: &str) -> String {
    format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>PHASE login failed</title><style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#1a1a1a;color:#eee;font:16px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}}main{{max-width:30rem;padding:2rem 2.25rem;border:1px solid rgba(238,238,238,.12);border-radius:16px;background:rgba(26,26,26,.96);box-shadow:0 18px 48px rgba(0,0,0,.35)}}h1{{margin:0 0 .5rem;color:#dc5050;font-size:1.4rem}}p{{margin:0;color:#eee}}code{{display:block;margin-top:1rem;padding:.85rem 1rem;border-radius:12px;background:rgba(255,255,255,.04);color:#eee;white-space:pre-wrap;word-break:break-word}}</style></head><body><main><h1>PHASE login failed</h1><p>Please try again.</p><code>{}</code></main></body></html>",
        html_escape(message)
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn decode_jwt_claims(access_token: &str) -> Option<serde_json::Value> {
    let mut parts = access_token.split('.');
    parts.next()?;
    let payload = parts.next()?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn claim_string(claims: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = claims.get(key) else {
            continue;
        };
        if let Some(text) = value.as_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if let Some(values) = value.as_array() {
            let items: Vec<_> = values
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .collect();
            if !items.is_empty() {
                return Some(items.join(", "));
            }
        }
    }
    None
}

fn first_non_empty_text<'a>(values: &[Option<&'a str>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    #[test]
    fn auth0_pkce_flow_uses_public_phase_client_audience_and_localhost_callback() {
        assert_eq!(AUTH0_CLIENT_ID, "26DJQIiBvq9Y4Ac3VZ0qeAbjdvYa9LEo");
        assert_eq!(AUTH0_DOMAIN, "polyhavenadmin.eu.auth0.com");
        assert_eq!(auth0_audience(), "https://admin.polyhaven.com/api/phase");
        assert_eq!(AUTH0_SCOPE, "openid profile email offline_access");
        assert_eq!(
            authorization_url(),
            "https://polyhavenadmin.eu.auth0.com/authorize"
        );
        assert_eq!(
            token_url(),
            "https://polyhavenadmin.eu.auth0.com/oauth/token"
        );
        assert_eq!(callback_url(), "http://127.0.0.1:45873/callback");
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorization_url_contains_pkce_and_phase_api_audience() {
        let url = build_authorization_url("verifier", "state value", callback_url());

        assert!(url.starts_with("https://polyhavenadmin.eu.auth0.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=26DJQIiBvq9Y4Ac3VZ0qeAbjdvYa9LEo"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A45873%2Fcallback"));
        assert!(url.contains("scope=openid%20profile%20email%20offline_access"));
        assert!(url.contains("audience=https%3A%2F%2Fadmin.polyhaven.com%2Fapi%2Fphase"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state%20value"));
    }

    #[test]
    fn parses_authorization_code_from_callback_request() {
        let request =
            "GET /callback?code=auth-code&state=expected HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

        assert_eq!(
            parse_callback_request(request, "expected").unwrap(),
            "auth-code"
        );
    }

    #[test]
    fn rejects_callback_with_wrong_state() {
        let request =
            "GET /callback?code=auth-code&state=wrong HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

        assert!(parse_callback_request(request, "expected")
            .unwrap_err()
            .to_string()
            .contains("state mismatch"));
    }

    #[test]
    fn debug_build_uses_local_phase_backend() {
        assert_eq!(phase_api_base_url(), "http://localhost:3001/");
    }

    #[test]
    fn token_expiry_from_expires_in_is_recorded_as_unix_seconds() {
        let tokens = tokens_from_response(
            TokenResponse {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                expires_in: Some(3600),
                token_type: "Bearer".into(),
                error: None,
                error_description: None,
            },
            10,
        )
        .unwrap();

        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token, "refresh");
        assert_eq!(tokens.expires_at, Some(3610));
    }

    #[test]
    fn logged_in_identity_prefers_standard_claims_and_role_arrays() {
        let payload = serde_json::json!({
            "name": "Ada Lovelace",
            "email": "ada@example.com",
            "https://admin.polyhaven.com/roles": ["admin", "editor"],
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{payload}.signature");

        let identity = logged_in_identity(&token).unwrap();
        assert_eq!(identity.name, "Ada Lovelace");
        assert_eq!(identity.role, "admin, editor");
    }

    #[test]
    fn logged_in_identity_falls_back_to_email_prefix_and_unknown_role() {
        let payload = serde_json::json!({
            "email": "ada@example.com",
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{payload}.signature");

        let identity = logged_in_identity(&token).unwrap();
        assert_eq!(identity.name, "ada");
        assert_eq!(identity.user_id, "Unknown");
        assert_eq!(identity.role, "Unknown");
    }

    #[test]
    fn logged_in_identity_uses_user_id_when_no_name_claim_is_present() {
        let payload = serde_json::json!({
            "sub": "auth0|abc123",
            "role": "editor",
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{payload}.signature");

        let identity = logged_in_identity(&token).unwrap();
        assert_eq!(identity.name, "auth0|abc123");
        assert_eq!(identity.user_id, "auth0|abc123");
        assert_eq!(identity.role, "editor");
    }

    #[test]
    fn fetch_logged_in_identity_uses_userinfo_name_when_available() {
        let payload = serde_json::json!({
            "sub": "auth0|abc123",
            "role": "editor",
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{payload}.signature");

        let userinfo = UserInfoResponse {
            sub: "auth0|abc123".into(),
            name: Some("Ada Lovelace".into()),
            preferred_username: None,
            nickname: None,
            email: Some("ada@example.com".into()),
        };
        let claims = decode_jwt_claims(&token).unwrap();
        let name = first_non_empty_text(&[
            userinfo.name.as_deref(),
            userinfo.preferred_username.as_deref(),
            userinfo.nickname.as_deref(),
            userinfo.email.as_deref(),
        ])
        .unwrap();
        assert_eq!(name, "Ada Lovelace");
        assert_eq!(
            claim_string(
                &claims,
                &[
                    "role",
                    "roles",
                    "https://admin.polyhaven.com/role",
                    "https://admin.polyhaven.com/roles",
                    "https://polyhaven.com/role",
                    "https://polyhaven.com/roles",
                ],
            ),
            Some("editor".into())
        );
    }
}
