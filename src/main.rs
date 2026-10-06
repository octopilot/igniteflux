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
use kube::runtime::events::{Event, EventType, Recorder, Reporter};
use kube::runtime::watcher;
use kube::{Api, Client, Resource as _, ResourceExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

const ANN_BOOTSTRAPPED: &str = "igniteflux.octopilot.io/bootstrapped"; // "<endpoint>@<unix time>"
const ANN_RERUN: &str = "igniteflux.octopilot.io/rerun"; // any value: bootstrap again, then removed

struct Ctx {
    client: Client,
    http: reqwest::Client,
    cfg: Config,
    target: Target,
    namespace: String,
    ar: ApiResource,
    recorder: Recorder,
}

/// Record what happened on the claim itself, so `kubectl describe` on it tells the story (and tests can watch for it).
async fn record(ctx: &Ctx, obj: &DynamicObject, type_: EventType, reason: &str, note: String) {
    let ev = Event {
        type_,
        reason: reason.to_string(),
        note: Some(note),
        action: "Bootstrap".to_string(),
        secondary: None,
    };
    if let Err(e) = ctx.recorder.publish(&ev, &obj.object_ref(&ctx.ar)).await {
        warn!(error = %e, "could not record event");
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0:#}")]
    Any(#[from] anyhow::Error),
}

fn condition_true(claim: &serde_json::Value, ty: &str) -> bool {
    claim
        .pointer("/status/conditions")
        .and_then(|c| c.as_array())
        .map(|cs| {
            cs.iter().any(|c| {
                c.get("type").and_then(|t| t.as_str()) == Some(ty)
                    && c.get("status").and_then(|s| s.as_str()) == Some("True")
            })
        })
        .unwrap_or(false)
}

/// The GitHub App: key from Secret Manager (via Workload Identity) when configured, otherwise from the Secret.
async fn load_app(ctx: &Ctx, token: &str) -> Result<github::App> {
    let git = &ctx.cfg.git;
    if let Some(secret) = &git.app_key_secret_manager {
        return Ok(github::App {
            app_id: git
                .app_id
                .clone()
                .ok_or_else(|| anyhow!("git.app_id is required with app_key_secret_manager"))?,
            installation_id: git.app_installation_id.clone().ok_or_else(|| {
                anyhow!("git.app_installation_id is required with app_key_secret_manager")
            })?,
            private_key_pem: gke::secret_manager_latest(&ctx.http, token, secret).await?,
        });
    }
    let (namespace, name) = (&ctx.namespace, &git.app_secret);
    let s = Api::<Secret>::namespaced(ctx.client.clone(), namespace)
        .get(name)
        .await
        .with_context(|| format!("secret {namespace}/{name}"))?;
    let d = s.data.unwrap_or_default();
    let get = |k: &str| -> Result<String> {
        Ok(String::from_utf8(
            d.get(k)
                .ok_or_else(|| anyhow!("secret lacks {k}"))?
                .0
                .clone(),
        )?)
    };
    Ok(github::App {
        app_id: get("githubAppID")?,
        installation_id: get("githubAppInstallationID")?,
        private_key_pem: get("githubAppPrivateKey")?,
    })
}

async fn reconcile(obj: Arc<DynamicObject>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let name = obj.name_any();
    let claim = serde_json::to_value(&*obj).map_err(anyhow::Error::from)?;
    let t = &ctx.target;

    if !condition_true(&claim, &t.ready_condition) {
        info!(kind = %t.kind, %name, "claim not {} yet", t.ready_condition);
        return Ok(Action::requeue(Duration::from_secs(60)));
    }

    let env = t
        .env_field
        .as_deref()
        .map(|f| config::resolve(f, &claim))
        .transpose()?
        .unwrap_or_default();

    // Where is the cluster?
    let token = gke::access_token(&ctx.http).await?;
    let (gke_info, endpoint) = match &t.cluster {
        ClusterAccess::Gke {
            project,
            location,
            name: cname,
        } => {
            let g = gke::describe(
                &ctx.http,
                &token,
                &config::resolve_in(project, &claim, &name, &env)?,
                &config::resolve_in(location, &claim, &name, &env)?,
                &config::resolve_in(cname, &claim, &name, &env)?,
            )
            .await?;
            if g.status != "RUNNING" {
                info!(%name, status = %g.status, "cluster not RUNNING");
                return Ok(Action::requeue(Duration::from_secs(60)));
            }
            let ep = g.endpoint.clone();
            (g, ep)
        }
    };
    let target_client = gke::client(&gke_info, &token).await?;

    // Already done for this very cluster (same endpoint) and Flux is healthy there? Then nothing to do.
    let rerun = obj.annotations().contains_key(ANN_RERUN);
    let done_for = obj
        .annotations()
        .get(ANN_BOOTSTRAPPED)
        .and_then(|v| v.split('@').next())
        .map(str::to_string);
    if !rerun
        && done_for.as_deref() == Some(endpoint.as_str())
        && apply::git_repository_ready(&target_client)
            .await
            .unwrap_or(false)
    {
        return Ok(Action::requeue(Duration::from_secs(600)));
    }
    info!(kind = %t.kind, %name, %endpoint, rerun, "bootstrapping Flux");
    record(
        &ctx,
        &obj,
        EventType::Normal,
        "Bootstrapping",
        format!("installing Flux into {endpoint} from {}", t.path),
    )
    .await;

    // Git checkout as the App, rendered with kustomize
    let app = load_app(&ctx, &token).await?;
    let gh_token = app.installation_token(&ctx.http).await?;
    let workdir = PathBuf::from(&ctx.cfg.workdir);
    let public = format!("https://github.com/{}.git", ctx.cfg.git.repository);
    tokio::task::block_in_place(|| {
        apply::checkout(
            &workdir,
            &github::App::clone_url(&ctx.cfg.git.repository, &gh_token),
            &public,
            &ctx.cfg.git.branch,
        )
    })?;
    let dir = workdir.join(config::expand(&t.path, &name, &env));
    let objs = tokio::task::block_in_place(|| apply::render(&dir))?;

    // Apply, credential, wait
    apply::app_secret(&target_client, &app).await?;
    apply::apply_all(&target_client, &objs).await?;
    apply::wait_git_repository(&target_client, Duration::from_secs(300)).await?;

    // Record on the claim; drop the rerun annotation
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(anyhow::Error::from)?
        .as_secs();
    let api: Api<DynamicObject> = Api::namespaced_with(
        ctx.client.clone(),
        obj.namespace().as_deref().unwrap_or("default"),
        &ctx.ar,
    );
    let patch = serde_json::json!({"metadata": {"annotations": {ANN_BOOTSTRAPPED: format!("{endpoint}@{now}"), ANN_RERUN: serde_json::Value::Null}}});
    api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(anyhow::Error::from)?;
    info!(kind = %t.kind, %name, %endpoint, objects = objs.len(), "Flux bootstrapped");
    record(
        &ctx,
        &obj,
        EventType::Normal,
        "Bootstrapped",
        format!(
            "Flux installed into {endpoint}: {} objects applied, GitRepository Ready",
            objs.len()
        ),
    )
    .await;
    Ok(Action::requeue(Duration::from_secs(600)))
}

fn error_policy(obj: Arc<DynamicObject>, err: &Error, ctx: Arc<Ctx>) -> Action {
    warn!(name = %obj.name_any(), error = %err, "reconcile failed");
    let note = err.to_string();
    tokio::spawn(
        async move { record(&ctx, &obj, EventType::Warning, "BootstrapFailed", note).await },
    );
    Action::requeue(Duration::from_secs(60))
}

/// Block until the API server serves `gvk`, then return its resource (with the real plural and scope).
async fn wait_for_kind(client: &Client, gvk: &GroupVersionKind) -> ApiResource {
    let mut logged = false;
    loop {
        match kube::discovery::pinned_kind(client, gvk).await {
            Ok((ar, _caps)) => return ar,
            Err(e) => {
                if !logged {
                    info!(kind = %gvk.kind, group = %gvk.group, error = %e, "kind not served yet; waiting for its CRD");
                    logged = true;
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    let cfg = Config::load(
        std::env::var("IGNITEFLUX_CONFIG").unwrap_or_else(|_| "/etc/igniteflux/config.yaml".into()),
    )?;
    let namespace = std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "flux-system".into());
    let client = Client::try_default().await?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    info!(targets = cfg.targets.len(), repo = %cfg.git.repository, "igniteflux starting");

    let mut runs = Vec::new();
    for target in cfg.targets.clone() {
        let (group, version) = target
            .api_version
            .split_once('/')
            .map(|(g, v)| (g.to_string(), v.to_string()))
            .unwrap_or(("".into(), target.api_version.clone()));
        let gvk = GroupVersionKind::gvk(&group, &version, &target.kind);
        let client = client.clone();
        let (http, cfg, namespace) = (http.clone(), cfg.clone(), namespace.clone());
        runs.push(tokio::spawn(async move {
            // The claim's CRD may not exist yet (Crossplane installs it after igniteflux starts): wait for the kind
            // to be served instead of failing every list with a 404.
            let ar = wait_for_kind(&client, &gvk).await;
            let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
            let ctx = Arc::new(Ctx {
                client: client.clone(),
                http: http.clone(),
                cfg: cfg.clone(),
                target: target.clone(),
                namespace: namespace.clone(),
                ar: ar.clone(),
                recorder: Recorder::new(
                    client.clone(),
                    Reporter {
                        controller: "igniteflux".into(),
                        instance: None,
                    },
                ),
            });
            info!(kind = %gvk.kind, "watching");
            Controller::new_with(api, watcher::Config::default(), ar)
                .run(reconcile, error_policy, ctx)
                .for_each(|r| async move {
                    if let Err(e) = r {
                        error!(error = ?e, "controller error");
                    }
                })
                .await
        }));
    }
    futures::future::join_all(runs).await;
    Ok(())
}
