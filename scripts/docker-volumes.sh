# Sourced by scripts/linux-docker-test.sh and scripts/crossui-linux-demo.sh
# (expects ROOT): names the Docker volumes holding the Linux cargo target dir
# and the SwiftPM scratch dir.
#
# On macOS every checkout shares one pair, like the dev Mac's single shared
# target dir (DESIGN.md R3). Elsewhere each checkout gets its own pair, keyed by
# a hash of its path, so worktrees that build at the same time on one host
# (agentbox) never share a target or .build dir. DWD_DOCKER_VOLUME_TAG
# overrides the key; set it to "" for the shared pair.
# The cargo registry and git volumes are always shared (cargo locks them).
: "${ROOT:?set ROOT to the checkout before sourcing docker-volumes.sh}"
if [[ -z "${DWD_DOCKER_VOLUME_TAG+set}" ]]; then
  if [[ "$(uname -s)" == Darwin ]]; then
    DWD_DOCKER_VOLUME_TAG=""
  elif command -v sha256sum >/dev/null 2>&1; then
    DWD_DOCKER_VOLUME_TAG="$(printf '%s' "$ROOT" | sha256sum | cut -c1-12)"
  else
    DWD_DOCKER_VOLUME_TAG="$(printf '%s' "$ROOT" | shasum -a 256 | cut -c1-12)"
  fi
fi
DWD_VOLUME_TARGET="dwd-linux-swift-target${DWD_DOCKER_VOLUME_TAG:+-$DWD_DOCKER_VOLUME_TAG}"
DWD_VOLUME_SWIFTPM="dwd-linux-swiftpm${DWD_DOCKER_VOLUME_TAG:+-$DWD_DOCKER_VOLUME_TAG}"
# Cargo jobs inside the container: DWD_LINUX_JOBS, else the host's CARGO_BUILD_JOBS, else 8.
DWD_DOCKER_JOBS="${DWD_LINUX_JOBS:-${CARGO_BUILD_JOBS:-8}}"
