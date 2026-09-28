# igniteflux

Bootstraps Flux into a cluster that a Kubernetes claim says exists.

`flux bootstrap` assumes a human at a terminal. In a platform where clusters are Crossplane claims, the moment a cluster is
Ready there is no human: igniteflux watches the claim kinds you name, and when one is Ready and its cluster is RUNNING it

1. builds a client for that cluster (GKE: Workload Identity token, `clusters.get`, done — no kubeconfig on disk),
2. checks out your Git repository as a GitHub App and `kustomize build`s the cluster's `flux-system` directory
   (your vendored Flux distribution and the cluster's own `GitRepository`/`Kustomization`),
3. server-side-applies it in two passes (CRDs first, then the CRs once they are established),
4. writes the App credential into `flux-system/flux-system` so the new cluster authenticates as the App,
5. waits for `GitRepository/flux-system` to be Ready, and
6. records `igniteflux.octopilot.io/bootstrapped: <endpoint>@<time>` on the claim.

If the cluster is replaced (new endpoint) it bootstraps again. Annotate the claim with `igniteflux.octopilot.io/rerun`
to force a run. Nothing Flux-specific is embedded: the manifests come from your repository, so the Flux version is whatever
you vendored, and upgrades are a Git change.

Runs as a Deployment where the claims live, under a ServiceAccount whose cloud identity may describe and administer the
target clusters. Configuration is one YAML file (`config/example.yaml`); `deploy/` has a manifest.

## Status

Working prototype born from microscaler/gcp-infrastructure, where a Job did this and needed a human every time a cluster was
recreated. GKE is the only `ClusterAccess` so far; the interface is one enum away from EKS/AKS.

## Build

    cargo build --release
    docker build -t ghcr.io/octopilot/igniteflux:dev .
