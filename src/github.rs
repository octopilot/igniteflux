//! GitHub App credentials: a JWT from the private key, an installation token, a clone URL.
use anyhow::{Context, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct App { pub app_id: String, pub installation_id: String, pub private_key_pem: String }

#[derive(Serialize)]
struct Claims { iat: u64, exp: u64, iss: String }
#[derive(Deserialize)]
struct InstallationToken { token: String }

impl App {
    fn jwt(&self) -> Result<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let claims = Claims { iat: now - 60, exp: now + 540, iss: self.app_id.clone() };
        let key = EncodingKey::from_rsa_pem(self.private_key_pem.as_bytes()).context("App private key is not RSA PEM")?;
        Ok(encode(&Header::new(Algorithm::RS256), &claims, &key)?)
    }

    /// Short-lived (1h) installation token: read on the repositories the App is installed on.
    pub async fn installation_token(&self, http: &reqwest::Client) -> Result<String> {
        let url = format!("https://api.github.com/app/installations/{}/access_tokens", self.installation_id);
        let t: InstallationToken = http.post(url).bearer_auth(self.jwt()?)
            .header("Accept", "application/vnd.github+json").header("User-Agent", "igniteflux")
            .send().await?.error_for_status().context("installation token")?.json().await?;
        Ok(t.token)
    }

    pub fn clone_url(repository: &str, token: &str) -> String {
        format!("https://x-access-token:{token}@github.com/{repository}.git")
    }
}
