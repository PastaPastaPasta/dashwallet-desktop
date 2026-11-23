# Combined Swift + Rust builder for Linux: builds the Rust core artifact
# bundle and runs the headless Swift tests. scripts/linux-docker-test.sh builds it
# as dwd-linux-swift:<hash of this file>, so an edit here makes a new image.
# GTK is intentionally absent (SwiftCrossUI targets are built by a separate
# image); use DWD_HEADLESS=1 with this image.
FROM swift:6.3.3-noble
ARG RUST_VERSION=1.98.1
ARG PROTOC_VERSION=29.3
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
# Ubuntu's `clang` package (it provides `cc` and libclang for Rust) repoints /usr/bin/clang and
# clang++ at clang-18. SwiftPM then hands that clang the toolchain-only flag -index-store-path
# when it builds C targets (SwiftCrossUI's dependencies, which SwiftCrossUIPatchTests brings
# into the headless graph), so the links go back to the Swift toolchain's clang after the
# install, as in Dockerfile.crossui.
RUN readlink -f /usr/bin/clang > /swift-clang && readlink -f /usr/bin/clang++ > /swift-clang++
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      libclang-dev clang libssl-dev pkg-config cmake make perl unzip curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && ln -sf "$(cat /swift-clang)" /usr/bin/clang && ln -sf "$(cat /swift-clang++)" /usr/bin/clang++ \
    && clang --version | head -1 && cc --version | head -1
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
        --component rustfmt --component clippy \
    && rustc --version && cargo --version
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
