# Run image for igniteflux: ubuntu:jammy plus git (checkout) and kustomize (render). Built and published by the manual
# publish-base-image workflow, used as runImage by the buildpack artifact in skaffold.yaml.
FROM ubuntu:jammy

# Explicit PATH: the CNB launcher builds the process PATH from the run image config.
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

# No CMD: ubuntu's CMD ["/bin/bash"] leaks into the app image and the CNB launcher then has no default process to start.
CMD []
