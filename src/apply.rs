//! Render with kustomize, server-side-apply in two passes (CRDs must be established before their CRs), wait for Flux.
use anyhow::{anyhow, bail, Context, Result};
use kube::api::{ApiResource, DynamicObject, GroupVersionKind, Patch, PatchParams};
use kube::discovery::{Discovery, Scope};
use kube::{Api, Client};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{debug, info};

const MANAGER: &str = "igniteflux";

/// `git clone --depth 1` (or fetch+reset) with a short-lived token that is never written to disk beyond the remote URL.
pub fn checkout(workdir: &Path, url: &str, public_url: &str, branch: &str) -> Result<()> {
    let ok = |c: &mut Command| -> Result<()> {
        let out = c.output().context("running git")?;
        if !out.status.success() { bail!("git failed: {}", String::from_utf8_lossy(&out.stderr)); }
        Ok(())
    };
    if workdir.join(".git").exists() {
        ok(Command::new("git").args(["-C", workdir.to_str().unwrap(), "remote", "set-url", "origin", url]))?;
        ok(Command::new("git").args(["-C", workdir.to_str().unwrap(), "fetch", "--depth", "1", "origin", branch]))?;
        ok(Command::new("git").args(["-C", workdir.to_str().unwrap(), "reset", "--hard", "FETCH_HEAD"]))?;
    } else {
        std::fs::create_dir_all(workdir.parent().unwrap_or(Path::new("/")))?;
        ok(Command::new("git").args(["clone", "-q", "--depth", "1", "--branch", branch, url, workdir.to_str().unwrap()]))?;
    }
    // the token must not stay on disk: point the remote back at the public URL
    ok(Command::new("git").args(["-C", workdir.to_str().unwrap(), "remote", "set-url", "origin", public_url]))?;
    Ok(())
}

pub fn render(dir: &Path) -> Result<Vec<DynamicObject>> {
    let out = Command::new("kustomize").args(["build", dir.to_str().unwrap()]).output().context("running kustomize")?;
    if !out.status.success() { bail!("kustomize build {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr)); }
    let mut objs = Vec::new();
    for doc in serde_yaml::Deserializer::from_slice(&out.stdout) {
        let v: serde_json::Value = serde::Deserialize::deserialize(doc)?;
        if v.is_null() { continue; }
        objs.push(serde_json::from_value::<DynamicObject>(v)?);
    }
    Ok(objs)
}

fn gvk(o: &DynamicObject) -> Result<GroupVersionKind> {
    let tm = o.types.as_ref().ok_or_else(|| anyhow!("object without apiVersion/kind"))?;
    let (group, version) = match tm.api_version.split_once('/') { Some((g, v)) => (g.to_string(), v.to_string()), None => ("".to_string(), tm.api_version.clone()) };
    Ok(GroupVersionKind::gvk(&group, &version, &tm.kind))
}

async fn apply_one(client: &Client, disc: &Discovery, o: &DynamicObject) -> Result<bool> {
    let gvk = gvk(o)?;
    let Some((ar, caps)) = disc.resolve_gvk(&gvk) else { return Ok(false) };
    let name = o.metadata.name.clone().ok_or_else(|| anyhow!("object without name"))?;
    let api: Api<DynamicObject> = match caps.scope {
        Scope::Cluster => Api::all_with(client.clone(), &ar),
        Scope::Namespaced => Api::namespaced_with(client.clone(), o.metadata.namespace.as_deref().unwrap_or("default"), &ar),
    };
    api.patch(&name, &PatchParams::apply(MANAGER).force(), &Patch::Apply(o)).await.with_context(|| format!("apply {}/{}", gvk.kind, name))?;
    debug!(kind = %gvk.kind, %name, "applied");
    Ok(true)
}

/// Apply everything the discovery can resolve, refresh discovery, apply the rest. Errors if anything remains unresolved.
pub async fn apply_all(client: &Client, objs: &[DynamicObject]) -> Result<()> {
    let mut pending: Vec<&DynamicObject> = objs.iter().collect();
    let deadline = Instant::now() + Duration::from_secs(180);
    while !pending.is_empty() {
        let disc = Discovery::new(client.clone()).run().await?;
        let mut next = Vec::new();
        for o in pending {
            if !apply_one(client, &disc, o).await? { next.push(o); }
        }
        if next.is_empty() { break; }
        if Instant::now() > deadline {
            let kinds: BTreeMap<String, usize> = next.iter().fold(BTreeMap::new(), |mut m, o| { *m.entry(o.types.as_ref().map(|t| t.kind.clone()).unwrap_or_default()).or_default() += 1; m });
            bail!("kinds never became available: {kinds:?}");
        }
        info!(remaining = next.len(), "waiting for CRDs to establish");
        tokio::time::sleep(Duration::from_secs(5)).await;
        pending = next;
    }
    Ok(())
}

/// `flux-system/flux-system` Secret with the GitHub App credential, so the GitRepository can authenticate as the App.
pub async fn app_secret(client: &Client, app: &crate::github::App) -> Result<()> {
    use k8s_openapi::api::core::v1::{Namespace, Secret};
    let ns: Api<Namespace> = Api::all(client.clone());
    let n = serde_json::from_value::<Namespace>(serde_json::json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"flux-system"}}))?;
    ns.patch("flux-system", &PatchParams::apply(MANAGER).force(), &Patch::Apply(&n)).await?;
    let sec: Api<Secret> = Api::namespaced(client.clone(), "flux-system");
    let s = serde_json::from_value::<Secret>(serde_json::json!({
        "apiVersion":"v1","kind":"Secret","metadata":{"name":"flux-system","namespace":"flux-system"},
        "stringData":{"githubAppID":app.app_id,"githubAppInstallationID":app.installation_id,"githubAppPrivateKey":app.private_key_pem}
    }))?;
    sec.patch("flux-system", &PatchParams::apply(MANAGER).force(), &Patch::Apply(&s)).await?;
    Ok(())
}

/// True when `GitRepository/flux-system` in flux-system reports Ready=True.
pub async fn git_repository_ready(client: &Client) -> Result<bool> {
    let ar = ApiResource::from_gvk(&GroupVersionKind::gvk("source.toolkit.fluxcd.io", "v1", "GitRepository"));
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), "flux-system", &ar);
    let Some(gr) = api.get_opt("flux-system").await? else { return Ok(false) };
    let ready = gr.data.pointer("/status/conditions").and_then(|c| c.as_array()).map(|cs| {
        cs.iter().any(|c| c.get("type").and_then(|t| t.as_str()) == Some("Ready") && c.get("status").and_then(|s| s.as_str()) == Some("True"))
    }).unwrap_or(false);
    Ok(ready)
}

pub async fn wait_git_repository(client: &Client, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if git_repository_ready(client).await? { return Ok(()); }
        if Instant::now() > deadline { bail!("GitRepository/flux-system not Ready within {timeout:?}"); }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
