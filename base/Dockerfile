FROM ubuntu:jammy

# Docker CLI (client only): the octopilot action mounts the runner docker socket into this container, so with a client
# present op can build docker: artifacts in the integration fan-out (before this, docker: artifacts only built with
# build_bypass, where op ran on the runner itself).
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    curl \
    jq \
    bash \
    ca-certificates \
    gnupg \
    && install -m 0755 -d /etc/apt/keyrings \
    && curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu jammy stable" > /etc/apt/sources.list.d/docker.list \
    && apt-get update && apt-get install -y --no-install-recommends docker-ce-cli docker-buildx-plugin \
    && rm -rf /var/lib/apt/lists/* \
    && curl -sL "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv5.6.0/kustomize_v5.6.0_linux_$(dpkg --print-architecture).tar.gz" | tar xz -C /usr/local/bin \
    && mkdir -p /var/lib/igniteflux && chmod 1777 /var/lib/igniteflux

# PATH=/cnb/process:/cnb/lifecycle: and failed with: failed to launch: bash exec: no such file or directory.
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
