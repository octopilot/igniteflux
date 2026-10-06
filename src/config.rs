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

fn default_ready() -> String {
    "Ready".into()
}

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
    /// Read the App private key from Google Secret Manager instead of `app_secret`, with the pod's Workload Identity
    /// (e.g. `projects/pw-ctl/secrets/flux-github-app-key`). Lets igniteflux run on a cluster with no secret sync.
    /// `app_id` and `app_installation_id` must then be set here, since they are not secret.
    #[serde(default)]
    pub app_key_secret_manager: Option<String>,
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub app_id: Option<String>,
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub app_installation_id: Option<String>,
}

/// Numeric IDs arrive as YAML numbers once a templating step drops the quotes (Helm toYaml, Flux substitution).
fn opt_string_or_number<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        S(String),
        N(u64),
    }
    Ok(Option::<V>::deserialize(d)?.map(|v| match v {
        V::S(s) => s,
        V::N(n) => n.to_string(),
    }))
}
fn default_branch() -> String {
    "main".into()
}
fn default_secret() -> String {
    "igniteflux-github-app".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub git: Git,
    pub targets: Vec<Target>,
    /// Where the checkout lives inside the pod.
    #[serde(default = "default_workdir")]
    pub workdir: String,
}
fn default_workdir() -> String {
    "/var/lib/igniteflux/repo".into()
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(serde_yaml::from_str(&text)?)
    }
}

/// Expand the `{name}` and `{env}` placeholders.
pub fn expand(spec: &str, name: &str, env: &str) -> String {
    spec.replace("{name}", name).replace("{env}", env)
}

/// `resolve`, then expand placeholders: `=runtime-{env}` gives `runtime-pp` for the pp claim.
pub fn resolve_in(spec: &str, claim: &serde_json::Value, name: &str, env: &str) -> Result<String> {
    Ok(expand(&resolve(spec, claim)?, name, env))
}

/// Resolve a config value: `=literal`, or a JSON pointer into the claim.
pub fn resolve(spec: &str, claim: &serde_json::Value) -> Result<String> {
    if let Some(lit) = spec.strip_prefix('=') {
        return Ok(lit.to_string());
    }
    let v = claim
        .pointer(spec)
        .with_context(|| format!("claim has no field {spec}"))?;
    Ok(match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string().trim_matches('"').to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn literal_with_placeholders() {
        let claim = json!({});
        assert_eq!(
            resolve_in("=runtime-{env}", &claim, "pp", "pp").unwrap(),
            "runtime-pp"
        );
        assert_eq!(resolve_in("=pw-ctl", &claim, "x", "").unwrap(), "pw-ctl");
    }

    #[test]
    fn pointer_and_missing_field() {
        let claim = json!({"spec": {"projectId": "pw-runtime-pp", "replicas": 3}});
        assert_eq!(resolve("/spec/projectId", &claim).unwrap(), "pw-runtime-pp");
        assert_eq!(resolve("/spec/replicas", &claim).unwrap(), "3");
        assert!(resolve("/spec/region", &claim).is_err());
    }

    #[test]
    fn path_expansion() {
        assert_eq!(
            expand("clusters/runtime/{env}/flux-system", "pp", "pp"),
            "clusters/runtime/pp/flux-system"
        );
        assert_eq!(expand("probes/{name}", "helm-test", ""), "probes/helm-test");
    }

    #[test]
    fn config_with_secret_manager_key() {
        let c: Config = serde_yaml::from_str(
            r#"
git:
  repository: microscaler/gcp-infrastructure
  app_key_secret_manager: projects/pw-ctl/secrets/flux-github-app-key
  app_id: 5097671
  app_installation_id: "165490781"
targets:
  - api_version: platform.pricewhisperer.ai/v1alpha1
    kind: RuntimeEnvironment
    cluster: { provider: gke, project: /spec/projectId, location: /spec/region, name: "=runtime-{env}" }
    env_field: /spec/environment
    path: clusters/runtime/{env}/flux-system
"#,
        )
        .unwrap();
        assert_eq!(c.git.branch, "main");
        assert_eq!(c.git.app_secret, "igniteflux-github-app");
        assert_eq!(
            c.git.app_key_secret_manager.as_deref(),
            Some("projects/pw-ctl/secrets/flux-github-app-key")
        );
        assert_eq!(c.git.app_id.as_deref(), Some("5097671"));
        assert_eq!(c.git.app_installation_id.as_deref(), Some("165490781"));
        assert_eq!(c.targets[0].ready_condition, "Ready");
        assert_eq!(c.workdir, "/var/lib/igniteflux/repo");
    }
}
