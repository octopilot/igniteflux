# Run image for igniteflux: the buildpack run image plus git (checkout) and kustomize (render). Built by op as a docker artifact
# and used as runImage by the buildpack artifact (the octopilot-pipeline-tools pattern).
FROM paketobuildpacks/run-jammy-base:latest
USER root
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && curl -sL "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv5.6.0/kustomize_v5.6.0_linux_$(dpkg --print-architecture).tar.gz" | tar xz -C /usr/local/bin \
 && mkdir -p /var/lib/igniteflux && chown cnb:cnb /var/lib/igniteflux
USER cnb
