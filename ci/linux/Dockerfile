# Linux builder for the Rust engine + Swift app (headless CI and local OrbStack builds).
FROM rust:1.98.1-bookworm
ARG PROTOC_VERSION=29.3
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      libclang-dev clang libssl-dev pkg-config cmake unzip curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN arch=$(uname -m | sed 's/aarch64/aarch_64/') \
    && curl -fsSL -o /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip" \
    && unzip -q /tmp/protoc.zip -d /usr/local && rm /tmp/protoc.zip && protoc --version
