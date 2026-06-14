use crate::config::{AccountRecord, ConfigStore, OAuthClientFile, normalize_email};
use crate::error::AppError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration as ChronoDuration, Utc};
use rand::{RngCore, rngs::OsRng};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use url::{Host, Url};

pub const CHAT_SPACES_READONLY: &str = "https://www.googleapis.com/auth/chat.spaces.readonly";
pub const CHAT_SPACES_CREATE: &str = "https://www.googleapis.com/auth/chat.spaces.create";
pub const CHAT_MESSAGES_READONLY: &str = "https://www.googleapis.com/auth/chat.messages.readonly";
pub const CHAT_MESSAGES_CREATE: &str = "https://www.googleapis.com/auth/chat.messages.create";
pub const CHAT_READSTATE_READONLY: &str =
    "https://www.googleapis.com/auth/chat.users.readstate.readonly";
pub const DEFAULT_OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:53682/callback";

pub const REQUIRED_SCOPES: [&str; 5] = [
    CHAT_SPACES_READONLY,
    CHAT_SPACES_CREATE,
    CHAT_MESSAGES_READONLY,
    CHAT_MESSAGES_CREATE,
    CHAT_READSTATE_READONLY,
];

#[derive(Debug, Clone)]
pub struct AccessToken {
    pub token: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

#[derive(Debug)]
struct OAuthCallback {
    code: String,
}

pub async fn add_account(store: &ConfigStore, email: &str) -> Result<(Value, Value), AppError> {
    let email = normalize_email(email);
    validate_email_like(&email, "auth.add")?;

    let credentials = store.load_oauth_client("auth.add")?;
    let redirect_url = oauth_redirect_url()?;
    let redirect_uri = redirect_url.as_str().to_string();
    let port = redirect_url.port().ok_or_else(|| {
        AppError::oauth(
            "auth.add",
            "OAuth redirect URI must include a port",
            json!({ "redirectUri": redirect_uri }),
        )
    })?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|error| {
            AppError::oauth(
                "auth.add",
                "failed to start OAuth loopback server",
                json!({
                    "redirectUri": redirect_uri,
                    "ioError": error.to_string()
                }),
            )
        })?;

    let state = random_urlsafe(32);
    let code_verifier = random_urlsafe(64);
    let code_challenge = pkce_challenge(&code_verifier);
    let auth_url = build_auth_url(&credentials, &redirect_uri, &state, &code_challenge)?;

    if let Err(error) = webbrowser::open(auth_url.as_str()) {
        return Err(AppError::oauth(
            "auth.add",
            "failed to open the OAuth URL in a browser",
            json!({
                "browserError": error.to_string(),
                "authUrl": auth_url.as_str()
            }),
        ));
    }

    let callback = wait_for_callback(listener, &state).await?;
    let token = exchange_authorization_code(
        &credentials,
        &redirect_uri,
        &callback.code,
        &code_verifier,
        "auth.add",
    )
    .await?;

    let refresh_token = token.refresh_token.ok_or_else(|| {
        AppError::oauth(
            "auth.add",
            "Google did not return a refresh token",
            json!({
                "fix": "remove this app from the Google account's third-party access list, then run `gchat auth add <email>` again"
            }),
        )
    })?;

    let now = Utc::now();
    let existing = store.load_account(&email, "auth.add").ok();
    let created_at = existing
        .as_ref()
        .map(|account| account.created_at.clone())
        .unwrap_or_else(|| now.to_rfc3339());
    let scopes = token.scope.as_deref().map(split_scope).unwrap_or_else(|| {
        REQUIRED_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect()
    });
    let expires_at = token
        .expires_in
        .map(|seconds| (now + ChronoDuration::seconds(seconds.max(0))).to_rfc3339());

    let account = AccountRecord {
        email: email.clone(),
        verified: false,
        scopes,
        token_storage: "pending".to_string(),
        refresh_token: None,
        expires_at,
        created_at,
        updated_at: now.to_rfc3339(),
    };
    let account = store.save_account_with_refresh(account, &refresh_token)?;

    Ok((
        json!({
            "email": account.email,
            "verified": account.verified,
            "scopes": account.scopes,
            "tokenStorage": account.token_storage,
            "configDir": store.root,
        }),
        json!({
            "browserOpened": true,
            "redirectUri": redirect_uri,
            "redirectPort": port,
            "storageMode": account.token_storage,
        }),
    ))
}

pub async fn access_token(
    store: &ConfigStore,
    email: &str,
    command: &str,
) -> Result<(AccountRecord, AccessToken), AppError> {
    let credentials = store.load_oauth_client(command)?;
    let account = store.load_account(email, command)?;
    let refresh_token = store.load_refresh_token(&account, command)?;
    let token = refresh_access_token(&credentials, &refresh_token, command).await?;
    let access_token = token.access_token.ok_or_else(|| {
        AppError::oauth(
            command,
            "Google did not return an access token",
            json!({ "email": account.email }),
        )
    })?;
    let scopes = token
        .scope
        .as_deref()
        .map(split_scope)
        .unwrap_or_else(|| account.scopes.clone());
    Ok((
        account,
        AccessToken {
            token: access_token,
            scopes,
        },
    ))
}

pub fn require_scope(scopes: &[String], required: &str, command: &str) -> Result<(), AppError> {
    if scopes.iter().any(|scope| scope == required) {
        return Ok(());
    }

    Err(AppError::missing_auth(
        command,
        "authenticated account is missing a required OAuth scope",
        json!({
            "requiredScope": required,
            "grantedScopes": scopes,
            "fix": "run `gchat auth add <email>` again and approve the requested scopes"
        }),
    ))
}

fn oauth_redirect_url() -> Result<Url, AppError> {
    let raw = std::env::var("GCHAT_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| DEFAULT_OAUTH_REDIRECT_URI.to_string());
    let url = Url::parse(&raw).map_err(|error| {
        AppError::oauth(
            "auth.add",
            "OAuth redirect URI is invalid",
            json!({ "redirectUri": raw, "urlError": error.to_string() }),
        )
    })?;

    if url.scheme() != "http" {
        return Err(AppError::oauth(
            "auth.add",
            "OAuth redirect URI must use http for local loopback",
            json!({ "redirectUri": url.as_str() }),
        ));
    }
    if url.port().is_none() {
        return Err(AppError::oauth(
            "auth.add",
            "OAuth redirect URI must include an explicit port",
            json!({ "redirectUri": url.as_str() }),
        ));
    }
    match url.host() {
        Some(Host::Ipv4(addr)) if addr.octets() == [127, 0, 0, 1] => {}
        Some(Host::Domain("localhost")) => {}
        _ => {
            return Err(AppError::oauth(
                "auth.add",
                "OAuth redirect URI must use localhost or 127.0.0.1",
                json!({ "redirectUri": url.as_str() }),
            ));
        }
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::oauth(
            "auth.add",
            "OAuth redirect URI must not include query parameters or a fragment",
            json!({ "redirectUri": url.as_str() }),
        ));
    }

    Ok(url)
}

async fn exchange_authorization_code(
    credentials: &OAuthClientFile,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
    command: &str,
) -> Result<TokenResponse, AppError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", credentials.client_id.as_str()),
        ("client_secret", credentials.client_secret.as_str()),
        ("code_verifier", code_verifier),
    ];
    post_token_form(credentials, &form, command).await
}

async fn refresh_access_token(
    credentials: &OAuthClientFile,
    refresh_token: &str,
    command: &str,
) -> Result<TokenResponse, AppError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", credentials.client_id.as_str()),
        ("client_secret", credentials.client_secret.as_str()),
    ];
    post_token_form(credentials, &form, command).await
}

async fn post_token_form(
    credentials: &OAuthClientFile,
    form: &[(&str, &str)],
    command: &str,
) -> Result<TokenResponse, AppError> {
    let response = Client::new()
        .post(&credentials.token_uri)
        .form(form)
        .send()
        .await
        .map_err(|error| {
            AppError::oauth(
                command,
                "failed to call Google's OAuth token endpoint",
                json!({ "networkError": error.to_string() }),
            )
        })?;

    let status = response.status();
    let body_text = response.text().await.map_err(|error| {
        AppError::oauth(
            command,
            "failed to read Google's OAuth token response",
            json!({ "networkError": error.to_string() }),
        )
    })?;
    let body: Value = serde_json::from_str(&body_text).unwrap_or_else(|_| {
        json!({
            "raw": body_text
        })
    });

    if !status.is_success() {
        let message = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("Google OAuth token endpoint returned an error");
        return Err(AppError::oauth(
            command,
            message,
            json!({ "status": status.as_u16(), "body": body }),
        ));
    }

    serde_json::from_value(body).map_err(|error| {
        AppError::oauth(
            command,
            "Google OAuth token response was malformed",
            json!({ "jsonError": error.to_string() }),
        )
    })
}

fn build_auth_url(
    credentials: &OAuthClientFile,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<Url, AppError> {
    let mut url = Url::parse(&credentials.auth_uri).map_err(|error| {
        AppError::oauth(
            "auth.add",
            "stored OAuth auth_uri is invalid",
            json!({ "urlError": error.to_string() }),
        )
    })?;

    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &credentials.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &REQUIRED_SCOPES.join(" "))
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256");

    Ok(url)
}

async fn wait_for_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<OAuthCallback, AppError> {
    let (mut stream, _) = timeout(Duration::from_secs(300), listener.accept())
        .await
        .map_err(|_| {
            AppError::oauth(
                "auth.add",
                "timed out waiting for OAuth browser callback",
                json!({ "timeoutSeconds": 300 }),
            )
        })?
        .map_err(|error| {
            AppError::oauth(
                "auth.add",
                "failed to accept OAuth browser callback",
                json!({ "ioError": error.to_string() }),
            )
        })?;

    let mut buffer = [0_u8; 8192];
    let bytes_read = timeout(Duration::from_secs(10), stream.read(&mut buffer))
        .await
        .map_err(|_| {
            AppError::oauth(
                "auth.add",
                "timed out reading OAuth browser callback",
                json!({ "timeoutSeconds": 10 }),
            )
        })?
        .map_err(|error| {
            AppError::oauth(
                "auth.add",
                "failed to read OAuth browser callback",
                json!({ "ioError": error.to_string() }),
            )
        })?;

    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let parsed = parse_callback_request(&request, expected_state);
    let response_body = if parsed.is_ok() {
        "Authentication complete. You can close this tab."
    } else {
        "Authentication failed. Return to the terminal for details."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response_body.len(),
        response_body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    parsed
}

fn parse_callback_request(request: &str, expected_state: &str) -> Result<OAuthCallback, AppError> {
    let first_line = request.lines().next().ok_or_else(|| {
        AppError::oauth("auth.add", "OAuth browser callback was empty", json!({}))
    })?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" || target.is_empty() {
        return Err(AppError::oauth(
            "auth.add",
            "OAuth browser callback was not a GET request",
            json!({ "method": method }),
        ));
    }

    let url = if target.starts_with("http://") || target.starts_with("https://") {
        Url::parse(target)
    } else {
        Url::parse(&format!("http://127.0.0.1{target}"))
    }
    .map_err(|error| {
        AppError::oauth(
            "auth.add",
            "OAuth browser callback URL was malformed",
            json!({ "urlError": error.to_string() }),
        )
    })?;

    let mut code = None;
    let mut state = None;
    let mut oauth_error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => oauth_error = Some(value.into_owned()),
            _ => {}
        }
    }

    if let Some(error) = oauth_error {
        return Err(AppError::oauth(
            "auth.add",
            "Google rejected the OAuth authorization request",
            json!({ "oauthError": error }),
        ));
    }

    if state.as_deref() != Some(expected_state) {
        return Err(AppError::oauth(
            "auth.add",
            "OAuth state did not match",
            json!({}),
        ));
    }

    let code = code.ok_or_else(|| {
        AppError::oauth(
            "auth.add",
            "OAuth callback did not include an authorization code",
            json!({}),
        )
    })?;

    Ok(OAuthCallback { code })
}

fn validate_email_like(email: &str, command: &str) -> Result<(), AppError> {
    if email.contains('@') && email.contains('.') && !email.contains(char::is_whitespace) {
        return Ok(());
    }
    Err(AppError::usage(
        command,
        "email address is malformed",
        json!({ "email": email }),
    ))
}

fn random_urlsafe(bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn split_scope(scope: &str) -> Vec<String> {
    scope
        .split_whitespace()
        .filter(|scope| !scope.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_callback() {
        let callback = parse_callback_request(
            "GET /callback?code=abc&state=state123 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            "state123",
        )
        .unwrap();
        assert_eq!(callback.code, "abc");
    }

    #[test]
    fn rejects_mismatched_callback_state() {
        let error = parse_callback_request(
            "GET /callback?code=abc&state=wrong HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            "state123",
        )
        .unwrap_err();
        assert_eq!(error.exit_code, 6);
    }
}
