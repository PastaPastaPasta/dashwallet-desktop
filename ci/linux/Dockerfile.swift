# Combined Swift + Rust builder for Linux: builds the Rust core artifact
# bundle and runs the headless Swift tests.
#   docker build -f ci/linux/Dockerfile.swift -t dwd-linux-swift ci/linux
# GTK is intentionally absent (SwiftCrossUI targets are built by a separate
# image); use DWD_HEADLESS=1 with this image.
FROM swift:6.3.3-noble
ARG RUST_VERSION=1.98.1
ARG PROTOC_VERSION=29.3
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      libclang-dev clang libssl-dev pkg-config cmake make perl unzip curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
        --component rustfmt --component clippy \
    && rustc --version && cargo --version
RUN arch=$(uname -m | sed 's/aarch64/aarch_64/') \
    && curl -fsSL -o /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${arch}.zip" \
    && unzip -q /tmp/protoc.zip -d /usr/local && rm /tmp/protoc.zip && protoc --version
