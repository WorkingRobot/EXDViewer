#[derive(Clone, Debug)]
pub struct GithubAuth {
    pub token: String,
    pub login: String,
}

/// SPA redirect path
pub const CALLBACK_PATH: &str = "/auth/github/callback";

pub struct AuthStart {
    pub url: String,
    /// PKCE verifier
    pub verifier: String,
    /// CSRF state
    pub state: String,
}

pub enum RelayResult {
    Code { code: String, state: String },
    Error(String),
}

pub use imp::{
    build_auth_start, exchange_code, fetch_client_id, relay_and_close, take_relayed_result,
};

#[cfg(target_arch = "wasm32")]
mod imp {
    use anyhow::{Result, anyhow, bail};
    use base64::Engine;
    use sha2::{Digest, Sha256};
    use web_sys::window;

    use super::{AuthStart, CALLBACK_PATH, GithubAuth, RelayResult};
    use crate::{
        github::GithubClient,
        utils::{fetch_url, request},
    };

    const SCOPE: &str = "public_repo";
    const RELAY_KEY: &str = "gh_oauth_result";

    fn b64url(bytes: impl AsRef<[u8]>) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn random_token(len: usize) -> String {
        let mut buf = vec![0u8; len];
        getrandom::fill(&mut buf).expect("getrandom failed");
        b64url(buf)
    }

    fn origin() -> Result<String> {
        window()
            .ok_or_else(|| anyhow!("no window"))?
            .location()
            .origin()
            .map_err(|e| anyhow!("no origin: {e:?}"))
    }

    fn api_base() -> Result<String> {
        Ok(format!("{}/api", origin()?))
    }

    fn redirect_uri() -> Result<String> {
        Ok(format!("{}{CALLBACK_PATH}", origin()?))
    }

    fn local_storage() -> Result<web_sys::Storage> {
        window()
            .ok_or_else(|| anyhow!("no window"))?
            .local_storage()
            .map_err(|e| anyhow!("localStorage unavailable: {e:?}"))?
            .ok_or_else(|| anyhow!("localStorage unavailable"))
    }

    pub async fn fetch_client_id() -> Result<String> {
        let config = fetch_url(format!("{}/github/oauth/config/", api_base()?)).await?;
        let config: serde_json::Value = serde_json::from_slice(&config)?;
        let client_id = config
            .get("client_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if client_id.is_empty() {
            bail!("GitHub sign-in is not configured on the server");
        }
        Ok(client_id)
    }

    pub fn build_auth_start(client_id: &str) -> Result<AuthStart> {
        let verifier = random_token(32);
        let state = random_token(16);
        let challenge = b64url(Sha256::digest(verifier.as_bytes()));

        let _ = local_storage().map(|s| s.remove_item(RELAY_KEY));

        let params =
            web_sys::UrlSearchParams::new().map_err(|e| anyhow!("UrlSearchParams: {e:?}"))?;
        params.append("client_id", client_id);
        params.append("redirect_uri", &redirect_uri()?);
        params.append("scope", SCOPE);
        params.append("state", &state);
        params.append("code_challenge", &challenge);
        params.append("code_challenge_method", "S256");

        Ok(AuthStart {
            url: format!(
                "https://github.com/login/oauth/authorize?{}",
                String::from(params.to_string())
            ),
            verifier,
            state,
        })
    }

    pub fn take_relayed_result() -> Option<RelayResult> {
        let store = local_storage().ok()?;
        let raw = store.get_item(RELAY_KEY).ok().flatten()?;
        let _ = store.remove_item(RELAY_KEY);

        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            let desc = value
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or(error);
            return Some(RelayResult::Error(desc.to_string()));
        }
        let code = value.get("code").and_then(|v| v.as_str())?.to_string();
        let state = value
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Some(RelayResult::Code { code, state })
    }

    pub async fn exchange_code(code: String, verifier: String) -> Result<GithubAuth> {
        let body = serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": redirect_uri()?,
        });
        let resp = request(
            "POST",
            format!("{}/github/oauth/token/", api_base()?),
            &[("Content-Type", "application/json")],
            Some(serde_json::to_vec(&body)?),
        )
        .await?;
        let json: serde_json::Value = serde_json::from_slice(&resp.bytes)?;

        if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
            let desc = json
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or(error);
            bail!("Sign-in failed: {desc}");
        }
        let token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("no access token in response"))?
            .to_string();

        let login = GithubClient::new(token.clone()).current_login().await?;
        Ok(GithubAuth { token, login })
    }

    pub fn relay_and_close() {
        let payload = build_relay_payload();
        if let Ok(store) = local_storage() {
            let _ = store.set_item(RELAY_KEY, &payload);
        }
        if let Some(win) = window() {
            let _ = win.close();
        }
    }

    fn build_relay_payload() -> String {
        let search = window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();
        let params = web_sys::UrlSearchParams::new_with_str(search.trim_start_matches('?'))
            .ok()
            .unwrap_or_else(|| web_sys::UrlSearchParams::new().unwrap());

        if let Some(error) = params.get("error") {
            let desc = params.get("error_description").unwrap_or(error);
            return serde_json::json!({ "error": "access_denied", "error_description": desc })
                .to_string();
        }
        serde_json::json!({
            "code": params.get("code").unwrap_or_default(),
            "state": params.get("state").unwrap_or_default(),
        })
        .to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use anyhow::{Result, bail};

    use super::{AuthStart, GithubAuth, RelayResult};

    pub async fn fetch_client_id() -> Result<String> {
        bail!("GitHub sign-in is only available in the web version")
    }

    pub fn build_auth_start(_client_id: &str) -> Result<AuthStart> {
        bail!("GitHub sign-in is only available in the web version")
    }

    pub fn take_relayed_result() -> Option<RelayResult> {
        None
    }

    pub async fn exchange_code(_code: String, _verifier: String) -> Result<GithubAuth> {
        bail!("GitHub sign-in is only available in the web version")
    }

    pub fn relay_and_close() {}
}
