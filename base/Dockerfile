# Run image for igniteflux (the octopilot-pipeline-tools base pattern): ubuntu:jammy plus git (checkout) and kustomize (render).
# Built by op as a docker artifact and used as runImage by the buildpack artifact.
FROM ghcr.io/octopilot/op-base:v1.1.2

# Explicit PATH: the CNB launcher takes the final PATH from the run image config; without this the launched process had
# PATH=/cnb/process:/cnb/lifecycle: and failed with: failed to launch: bash exec: no such file or directory.
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    curl \
    jq \
    bash \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
 && curl -sL "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv5.6.0/kustomize_v5.6.0_linux_$(dpkg --print-architecture).tar.gz" | tar xz -C /usr/local/bin \
 && mkdir -p /var/lib/igniteflux && chmod 1777 /var/lib/igniteflux
