# Linux builder for the Rust engine + Swift app (headless CI and local OrbStack builds).
# The WebKitGTK development packages are what `cargo build --workspace` needs for the Tauri crate
# rust/crates/dw-app (G-01; the list in apps/desktop/README.md).
FROM rust:1.98.1-bookworm
ARG PROTOC_VERSION=29.3
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      libclang-dev clang libssl-dev pkg-config cmake unzip curl ca-certificates \
      libwebkit2gtk-4.1-dev build-essential wget file libxdo-dev libayatana-appindicator3-dev librsvg2-dev \
    && rm -rf /var/lib/apt/lists/*
# The zip's SHA-256 is checked (the 29.3 values, as in ci/github/install-protoc.sh; another
# PROTOC_VERSION fails the check until its hashes are added).
RUN arch=$(uname -m | sed 's/aarch64/aarch_64/') \
    && case "$arch" in \
         x86_64) sha256=3e866620c5be27664f3d2fa2d656b5f3e09b5152b42f1bedbf427b333e90021a ;; \
         aarch_64) sha256=6427349140e01f06e049e707a58709a4f221ae73ab9a0425bc4a00c8d0e1ab32 ;; \
       esac \
    && curl -fsSL -o /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip" \
    && echo "${sha256:-unknown}  /tmp/protoc.zip" | sha256sum -c - \
    && unzip -q /tmp/protoc.zip -d /usr/local && rm /tmp/protoc.zip && protoc --version
