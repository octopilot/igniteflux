//! Reaching a GKE cluster from a Workload-Identity pod: a metadata-server token, a `clusters.get`, a kubeconfig.
use anyhow::{Context, Result};
use kube::config::{AuthInfo, Cluster, Context as KContext, KubeConfigOptions, Kubeconfig, NamedAuthInfo, NamedCluster, NamedContext};
use serde::Deserialize;

const METADATA: &str = "http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/token";

#[derive(Deserialize)]
struct Token { access_token: String }

/// Access token for the pod's Workload Identity GSA.
pub async fn access_token(http: &reqwest::Client) -> Result<String> {
    let t: Token = http.get(METADATA).header("Metadata-Flavor", "Google").send().await?.error_for_status()?.json().await?;
    Ok(t.access_token)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MasterAuth { cluster_ca_certificate: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClusterInfo { endpoint: String, status: String, master_auth: MasterAuth }

pub struct Gke { pub endpoint: String, pub status: String, pub ca_b64: String }

/// `container.googleapis.com` describe; works for zonal and regional clusters (location is either).
pub async fn describe(http: &reqwest::Client, token: &str, project: &str, location: &str, name: &str) -> Result<Gke> {
    let url = format!("https://container.googleapis.com/v1/projects/{project}/locations/{location}/clusters/{name}");
    let c: ClusterInfo = http.get(url).bearer_auth(token).send().await?.error_for_status().context("clusters.get")?.json().await?;
    Ok(Gke { endpoint: c.endpoint, status: c.status, ca_b64: c.master_auth.cluster_ca_certificate })
}

/// A kube client for the target, authenticated with the same token (GKE accepts Google access tokens directly).
pub async fn client(gke: &Gke, token: &str) -> Result<kube::Client> {
    let kc = Kubeconfig {
        clusters: vec![NamedCluster { name: "target".into(), cluster: Some(Cluster { server: Some(format!("https://{}", gke.endpoint)), certificate_authority_data: Some(gke.ca_b64.clone()), ..Default::default() }) }],
        auth_infos: vec![NamedAuthInfo { name: "wi".into(), auth_info: Some(AuthInfo { token: Some(token.to_string().into()), ..Default::default() }) }],
        contexts: vec![NamedContext { name: "target".into(), context: Some(KContext { cluster: "target".into(), user: Some("wi".into()), ..Default::default() }) }],
        current_context: Some("target".into()),
        ..Default::default()
    };
    let cfg = kube::Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default()).await?;
    Ok(kube::Client::try_from(cfg)?)
}
