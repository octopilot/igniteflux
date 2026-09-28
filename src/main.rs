//! igniteflux: when a Kubernetes claim says a cluster exists and is Ready, install Flux into it from a Git checkout
//! and hand it its own sync. Idempotent, re-runs when the cluster is replaced, records what it did on the claim.
mod apply;
mod config;
mod github;
mod gke;

use anyhow::{anyhow, Context as _, Result};
use config::{ClusterAccess, Config, Target};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::api::{ApiResource, DynamicObject, GroupVersionKind, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher;
use kube::{Api, Client, ResourceExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

const ANN_BOOTSTRAPPED: &str = "igniteflux.octopilot.io/bootstrapped";   // "<endpoint>@<unix time>"
const ANN_RERUN: &str = "igniteflux.octopilot.io/rerun";                 // any value: bootstrap again, then removed

struct Ctx { client: Client, http: reqwest::Client, cfg: Config, target: Target, namespace: String, ar: ApiResource }

#[derive(Debug, thiserror::Error)]
enum Error { #[error("{0:#}")] Any(#[from] anyhow::Error) }

fn condition_true(claim: &serde_json::Value, ty: &str) -> bool {
    claim.pointer("/status/conditions").and_then(|c| c.as_array()).map(|cs| cs.iter().any(|c|
        c.get("type").and_then(|t| t.as_str()) == Some(ty) && c.get("status").and_then(|s| s.as_str()) == Some("True"))).unwrap_or(false)
}

async fn load_app(client: &Client, namespace: &str, name: &str) -> Result<github::App> {
    let s = Api::<Secret>::namespaced(client.clone(), namespace).get(name).await.with_context(|| format!("secret {namespace}/{name}"))?;
    let d = s.data.unwrap_or_default();
    let get = |k: &str| -> Result<String> { Ok(String::from_utf8(d.get(k).ok_or_else(|| anyhow!("secret lacks {k}"))?.0.clone())?) };
    Ok(github::App { app_id: get("githubAppID")?, installation_id: get("githubAppInstallationID")?, private_key_pem: get("githubAppPrivateKey")? })
}

async fn reconcile(obj: Arc<DynamicObject>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let name = obj.name_any();
    let claim = serde_json::to_value(&*obj).map_err(anyhow::Error::from)?;
    let t = &ctx.target;

    if !condition_true(&claim, &t.ready_condition) {
        info!(kind = %t.kind, %name, "claim not {} yet", t.ready_condition);
        return Ok(Action::requeue(Duration::from_secs(60)));
    }

    // Where is the cluster?
    let token = gke::access_token(&ctx.http).await?;
    let (gke_info, endpoint) = match &t.cluster {
        ClusterAccess::Gke { project, location, name: cname } => {
            let g = gke::describe(&ctx.http, &token, &config::resolve(project, &claim)?, &config::resolve(location, &claim)?, &config::resolve(cname, &claim)?).await?;
            if g.status != "RUNNING" { info!(%name, status = %g.status, "cluster not RUNNING"); return Ok(Action::requeue(Duration::from_secs(60))); }
            let ep = g.endpoint.clone(); (g, ep)
        }
    };
    let target_client = gke::client(&gke_info, &token).await?;

    // Already done for this very cluster (same endpoint) and Flux is healthy there? Then nothing to do.
    let rerun = obj.annotations().contains_key(ANN_RERUN);
    let done_for = obj.annotations().get(ANN_BOOTSTRAPPED).and_then(|v| v.split('@').next()).map(str::to_string);
    if !rerun && done_for.as_deref() == Some(endpoint.as_str()) && apply::git_repository_ready(&target_client).await.unwrap_or(false) {
        return Ok(Action::requeue(Duration::from_secs(600)));
    }
    info!(kind = %t.kind, %name, %endpoint, rerun, "bootstrapping Flux");

    // Git checkout as the App, rendered with kustomize
    let app = load_app(&ctx.client, &ctx.namespace, &ctx.cfg.git.app_secret).await?;
    let gh_token = app.installation_token(&ctx.http).await?;
    let workdir = PathBuf::from(&ctx.cfg.workdir);
    let public = format!("https://github.com/{}.git", ctx.cfg.git.repository);
    tokio::task::block_in_place(|| apply::checkout(&workdir, &github::App::clone_url(&ctx.cfg.git.repository, &gh_token), &public, &ctx.cfg.git.branch))?;
    let env = t.env_field.as_deref().map(|f| config::resolve(f, &claim)).transpose()?.unwrap_or_default();
    let dir = workdir.join(t.path.replace("{name}", &name).replace("{env}", &env));
    let objs = tokio::task::block_in_place(|| apply::render(&dir))?;

    // Apply, credential, wait
    apply::app_secret(&target_client, &app).await?;
    apply::apply_all(&target_client, &objs).await?;
    apply::wait_git_repository(&target_client, Duration::from_secs(300)).await?;

    // Record on the claim; drop the rerun annotation
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(anyhow::Error::from)?.as_secs();
    let api: Api<DynamicObject> = Api::namespaced_with(ctx.client.clone(), obj.namespace().as_deref().unwrap_or("default"), &ctx.ar);
    let patch = serde_json::json!({"metadata": {"annotations": {ANN_BOOTSTRAPPED: format!("{endpoint}@{now}"), ANN_RERUN: serde_json::Value::Null}}});
    api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch)).await.map_err(anyhow::Error::from)?;
    info!(kind = %t.kind, %name, %endpoint, objects = objs.len(), "Flux bootstrapped");
    Ok(Action::requeue(Duration::from_secs(600)))
}

fn error_policy(obj: Arc<DynamicObject>, err: &Error, _ctx: Arc<Ctx>) -> Action {
    warn!(name = %obj.name_any(), error = %err, "reconcile failed");
    Action::requeue(Duration::from_secs(60))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?)).init();
    let cfg = Config::load(std::env::var("IGNITEFLUX_CONFIG").unwrap_or_else(|_| "/etc/igniteflux/config.yaml".into()))?;
    let namespace = std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "flux-system".into());
    let client = Client::try_default().await?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    info!(targets = cfg.targets.len(), repo = %cfg.git.repository, "igniteflux starting");

    let mut runs = Vec::new();
    for target in cfg.targets.clone() {
        let (group, version) = target.api_version.split_once('/').map(|(g, v)| (g.to_string(), v.to_string())).unwrap_or(("".into(), target.api_version.clone()));
        let ar = ApiResource::from_gvk(&GroupVersionKind::gvk(&group, &version, &target.kind));
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
        let ctx = Arc::new(Ctx { client: client.clone(), http: http.clone(), cfg: cfg.clone(), target: target.clone(), namespace: namespace.clone(), ar: ar.clone() });
        let ctrl = Controller::new_with(api, watcher::Config::default(), ar).run(reconcile, error_policy, ctx)
            .for_each(|r| async move { if let Err(e) = r { error!(error = ?e, "controller error"); } });
        runs.push(tokio::spawn(ctrl));
    }
    futures::future::join_all(runs).await;
    Ok(())
}
