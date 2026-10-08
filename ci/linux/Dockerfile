# Linux builder for the Rust engine + Swift app (headless CI and local OrbStack builds).
# The WebKitGTK development packages are what `cargo build --workspace` needs for the Tauri crate
# rust/crates/dw-app (G-01; the list in apps/desktop/README.md).
FROM rust:1.98.1-bookworm
ARG PROTOC_VERSION=29.3
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      libclang-dev clang libssl-dev pkg-config cmake unzip curl ca-certificates \
      libwebkit2gtk-4.1-dev build-essential wget file libxdo-dev libayatana-appindicator3-dev librsvg2-dev \
    && rm -rf /var/lib/apt/lists/*
RUN arch=$(uname -m | sed 's/aarch64/aarch_64/') \
    && curl -fsSL -o /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip" \
    && unzip -q /tmp/protoc.zip -d /usr/local && rm /tmp/protoc.zip && protoc --version
