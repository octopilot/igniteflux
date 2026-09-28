//! What to watch and how to reach the clusters it describes.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// One claim kind to watch. Field paths are JSON pointers into the claim (e.g. `/status/clusterEndpoint`).
#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    /// e.g. `platform.pricewhisperer.ai/v1alpha1`
    pub api_version: String,
    /// e.g. `HubCluster`
    pub kind: String,
    /// Conditions of this type must be True before bootstrapping (default `Ready`).
    #[serde(default = "default_ready")]
    pub ready_condition: String,
    /// How to reach the cluster the claim created.
    pub cluster: ClusterAccess,
    /// Directory in the Git checkout to `kustomize build` and apply (may contain `{name}` and `{env}` placeholders
    /// resolved from the claim's metadata.name and the `env_field`).
    pub path: String,
    /// Optional JSON pointer to a field used for the `{env}` placeholder.
    #[serde(default)]
    pub env_field: Option<String>,
}

fn default_ready() -> String { "Ready".into() }

/// How to find and authenticate to the target cluster.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum ClusterAccess {
    /// GKE: describe the cluster with a Workload Identity token, connect with the same token.
    Gke {
        /// JSON pointers into the claim, or literal values prefixed with `=` (e.g. `=pw-ctl`).
        project: String,
        location: String,
        name: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Git {
    /// `owner/repo`
    pub repository: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Secret in the controller's namespace with keys githubAppID, githubAppInstallationID, githubAppPrivateKey.
    /// The same three keys are written to the target's `flux-system/flux-system` secret.
    #[serde(default = "default_secret")]
    pub app_secret: String,
}
fn default_branch() -> String { "main".into() }
fn default_secret() -> String { "igniteflux-github-app".into() }

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub git: Git,
    pub targets: Vec<Target>,
    /// Where the checkout lives inside the pod.
    #[serde(default = "default_workdir")]
    pub workdir: String,
}
fn default_workdir() -> String { "/var/lib/igniteflux/repo".into() }

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref()).with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(serde_yaml::from_str(&text)?)
    }
}

/// Resolve a config value: `=literal`, or a JSON pointer into the claim.
pub fn resolve(spec: &str, claim: &serde_json::Value) -> Result<String> {
    if let Some(lit) = spec.strip_prefix('=') {
        return Ok(lit.to_string());
    }
    let v = claim.pointer(spec).with_context(|| format!("claim has no field {spec}"))?;
    Ok(match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string().trim_matches('"').to_string(),
    })
}
