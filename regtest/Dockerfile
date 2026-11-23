# Regtest image: official Dash Core release binaries (SHA-256 pinned in scripts/fetch-dashcore.sh)
# plus the vendored Dash Core functional test_framework and its one native dependency (dash_hash).
#
# Build context: regtest
#   docker compose -f regtest/docker-compose.yml build
# Architectures: linux/amd64 (x86_64-linux-gnu tarball) and linux/arm64 (aarch64-linux-gnu tarball);
# the triple is chosen from `uname -m` inside the build, so a native build on either host works.
ARG BASE_IMAGE=python:3.12-slim

FROM ${BASE_IMAGE} AS dashhash
# dash_hash provides X11 for test_framework/messages.py. Built from the pinned tag archive.
ARG DASH_HASH_VERSION=1.4.0
ARG DASH_HASH_SHA256=2490feb0bb186a03956a140bb077d196fa4e2b0c22c5df0517c5f1bbd6cc7e5a
RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends gcc libc6-dev curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL --retry 3 -o /tmp/dash_hash.tar.gz \
        "https://github.com/dashpay/dash_hash/archive/refs/tags/${DASH_HASH_VERSION}.tar.gz" \
    && echo "${DASH_HASH_SHA256}  /tmp/dash_hash.tar.gz" | sha256sum -c - \
    && pip wheel --no-deps --wheel-dir /wheels /tmp/dash_hash.tar.gz

FROM ${BASE_IMAGE}
RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=dashhash /wheels /wheels
RUN pip install --no-cache-dir --no-deps /wheels/*.whl && rm -rf /wheels \
    && python3 -c "import dash_hash"

COPY scripts/fetch-dashcore.sh /usr/local/bin/fetch-dashcore.sh
RUN DASHCORE_CACHE=/tmp/dashcore-cache sh /usr/local/bin/fetch-dashcore.sh /opt/dashcore \
    && rm -rf /tmp/dashcore-cache \
    && /opt/dashcore/bin/dashd -datadir=/tmp -version > /tmp/dashd-version \
    && head -1 /tmp/dashd-version && rm /tmp/dashd-version
ENV PATH=/opt/dashcore/bin:$PATH

COPY functional /opt/dwd-functional
COPY scripts/node-entrypoint.sh /usr/local/bin/node-entrypoint.sh

RUN useradd --create-home --uid 1000 dash
USER dash
WORKDIR /home/dash
# 19898 = regtest RPC, 19899 = regtest P2P
EXPOSE 19898 19899
ENTRYPOINT ["/usr/local/bin/node-entrypoint.sh"]
