use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::{form_urlencoded, Url};

use crate::fs::models::FsError;

const CALLBACK_ADDR: &str = "127.0.0.1:53682";
const CALLBACK_PATH: &str = "/oauth/callback";
const CALLBACK_URL: &str = "http://127.0.0.1:53682/oauth/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy)]
enum OAuthProvider {
    GoogleDrive,
    OneDrive,
    Dropbox,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthTokenResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[tauri::command]
pub async fn create_oauth_tokens(
    provider: String,
    client_id: String,
    client_secret: Option<String>,
) -> Result<OAuthTokenResult, FsError> {
    let provider = OAuthProvider::parse(&provider)?;
    let client_id = client_id.trim().to_string();
    let client_secret = client_secret
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if client_id.is_empty() {
        return Err(oauth_error(
            "oauth_client_id_required",
            "OAuth client ID is required.",
        ));
    }

    let listener = TcpListener::bind(CALLBACK_ADDR).map_err(|error| {
        oauth_error(
            "oauth_callback_unavailable",
            format!(
                "Unable to listen on {CALLBACK_URL}. Close the app using that port and try again: {error}"
            ),
        )
    })?;

    let state = random_token(32);
    let verifier = random_token(96);
    let challenge = pkce_challenge(&verifier);
    let authorization_url = provider.authorization_url(&client_id, &state, &challenge)?;

    tauri_plugin_opener::open_url(authorization_url.as_str(), None::<&str>).map_err(|error| {
        oauth_error(
            "oauth_browser_open_failed",
            format!("Unable to open the OAuth authorization page: {error}"),
        )
    })?;

    let code = tauri::async_runtime::spawn_blocking(move || wait_for_callback(listener, state))
        .await
        .map_err(|error| {
            oauth_error(
                "oauth_callback_failed",
                format!("OAuth callback task failed: {error}"),
            )
        })??;

    exchange_authorization_code(
        provider,
        &client_id,
        client_secret.as_deref(),
        &verifier,
        &code,
    )
    .await
}

impl OAuthProvider {
    fn parse(provider: &str) -> Result<Self, FsError> {
        match provider.to_ascii_lowercase().as_str() {
            "gdrive" | "google" | "google_drive" => Ok(Self::GoogleDrive),
            "onedrive" | "one_drive" => Ok(Self::OneDrive),
            "dropbox" => Ok(Self::Dropbox),
            _ => Err(oauth_error(
                "oauth_provider_unsupported",
                format!("OAuth token creation is not supported for '{provider}'."),
            )),
        }
    }

    fn authorization_endpoint(self) -> &'static str {
        match self {
            Self::GoogleDrive => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::OneDrive => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            Self::Dropbox => "https://www.dropbox.com/oauth2/authorize",
        }
    }

    fn token_endpoint(self) -> &'static str {
        match self {
            Self::GoogleDrive => "https://oauth2.googleapis.com/token",
            Self::OneDrive => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            Self::Dropbox => "https://api.dropboxapi.com/oauth2/token",
        }
    }

    fn scope(self) -> &'static str {
        match self {
            Self::GoogleDrive => "https://www.googleapis.com/auth/drive",
            Self::OneDrive => "offline_access Files.ReadWrite",
            Self::Dropbox => "files.metadata.read files.content.read files.content.write",
        }
    }

    fn authorization_url(
        self,
        client_id: &str,
        state: &str,
        challenge: &str,
    ) -> Result<Url, FsError> {
        let mut url = Url::parse(self.authorization_endpoint()).map_err(|error| {
            oauth_error(
                "oauth_authorization_url_invalid",
                format!("Unable to build authorization URL: {error}"),
            )
        })?;

        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", CALLBACK_URL)
            .append_pair("response_type", "code")
            .append_pair("scope", self.scope())
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");

        match self {
            Self::GoogleDrive => {
                url.query_pairs_mut()
                    .append_pair("access_type", "offline")
                    .append_pair("prompt", "consent")
                    .append_pair("include_granted_scopes", "true");
            }
            Self::OneDrive => {
                url.query_pairs_mut()
                    .append_pair("prompt", "select_account");
            }
            Self::Dropbox => {
                url.query_pairs_mut()
                    .append_pair("token_access_type", "offline");
            }
        }

        Ok(url)
    }
}

async fn exchange_authorization_code(
    provider: OAuthProvider,
    client_id: &str,
    client_secret: Option<&str>,
    verifier: &str,
    code: &str,
) -> Result<OAuthTokenResult, FsError> {
    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", CALLBACK_URL.to_string()),
        ("client_id", client_id.to_string()),
        ("code_verifier", verifier.to_string()),
    ];

    if let Some(client_secret) = client_secret {
        params.push(("client_secret", client_secret.to_string()));
    }

    let response = reqwest::Client::new()
        .post(provider.token_endpoint())
        .form(&params)
        .send()
        .await
        .map_err(|error| {
            oauth_error(
                "oauth_token_request_failed",
                format!("OAuth token request failed: {error}"),
            )
        })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        oauth_error(
            "oauth_token_response_failed",
            format!("Unable to read OAuth token response: {error}"),
        )
    })?;

    let parsed: ProviderTokenResponse = serde_json::from_str(&body).map_err(|error| {
        oauth_error(
            "oauth_token_response_invalid",
            format!("OAuth token response was not valid JSON: {error}"),
        )
    })?;

    if !status.is_success() || parsed.error.is_some() {
        let detail = parsed
            .error_description
            .or(parsed.error)
            .unwrap_or_else(|| format!("OAuth provider returned HTTP {status}."));
        return Err(oauth_error("oauth_token_exchange_failed", detail));
    }

    let access_token = parsed.access_token.ok_or_else(|| {
        oauth_error(
            "oauth_access_token_missing",
            "OAuth provider did not return an access token.",
        )
    })?;

    Ok(OAuthTokenResult {
        access_token,
        refresh_token: parsed.refresh_token,
        expires_in: parsed.expires_in,
        token_type: parsed.token_type,
        scope: parsed.scope,
    })
}

fn wait_for_callback(listener: TcpListener, expected_state: String) -> Result<String, FsError> {
    listener.set_nonblocking(true).map_err(|error| {
        oauth_error(
            "oauth_callback_failed",
            format!("Unable to configure OAuth callback listener: {error}"),
        )
    })?;

    let deadline = Instant::now() + CALLBACK_TIMEOUT;

    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).map_err(|error| {
                    oauth_error(
                        "oauth_callback_failed",
                        format!("Unable to read OAuth callback: {error}"),
                    )
                })?;

                let request = String::from_utf8_lossy(&buffer[..bytes]);
                let result = parse_callback_request(&request, &expected_state);

                match &result {
                    Ok(_) => write_callback_response(&mut stream, true),
                    Err(error) => {
                        write_callback_response(&mut stream, false);
                        if error.code == "oauth_callback_ignored" {
                            continue;
                        }
                    }
                }

                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(oauth_error(
                        "oauth_callback_timeout",
                        "OAuth authorization timed out before the browser returned a code.",
                    ));
                }

                std::thread::sleep(Duration::from_millis(80));
            }
            Err(error) => {
                return Err(oauth_error(
                    "oauth_callback_failed",
                    format!("OAuth callback listener failed: {error}"),
                ));
            }
        }
    }
}

fn parse_callback_request(request: &str, expected_state: &str) -> Result<String, FsError> {
    let Some(first_line) = request.lines().next() else {
        return Err(oauth_error(
            "oauth_callback_invalid",
            "OAuth callback was empty.",
        ));
    };

    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method != "GET" || target.is_empty() {
        return Err(oauth_error(
            "oauth_callback_invalid",
            "OAuth callback request was invalid.",
        ));
    }

    let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|error| {
        oauth_error(
            "oauth_callback_invalid",
            format!("OAuth callback URL was invalid: {error}"),
        )
    })?;

    if url.path() != CALLBACK_PATH {
        return Err(oauth_error(
            "oauth_callback_ignored",
            "OAuth callback path did not match.",
        ));
    }

    let query: std::collections::HashMap<String, String> =
        form_urlencoded::parse(url.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();

    if let Some(error) = query.get("error") {
        let detail = query
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        return Err(oauth_error("oauth_authorization_failed", detail));
    }

    if query.get("state").map(String::as_str) != Some(expected_state) {
        return Err(oauth_error(
            "oauth_state_mismatch",
            "OAuth callback state did not match the request.",
        ));
    }

    query.get("code").cloned().ok_or_else(|| {
        oauth_error(
            "oauth_code_missing",
            "OAuth callback did not include an authorization code.",
        )
    })
}

fn write_callback_response(stream: &mut std::net::TcpStream, success: bool) {
    let body = if success {
        "Carelo received the authorization code. You can close this tab and return to the app."
    } else {
        "Carelo could not complete authorization. Return to the app for details."
    };
    let status = if success { "200 OK" } else { "400 Bad Request" };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn random_token(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), len)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn oauth_error(code: impl Into<String>, message: impl Into<String>) -> FsError {
    FsError::new(code, message, None)
}
