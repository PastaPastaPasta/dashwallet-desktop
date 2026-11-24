#!/usr/bin/env bash
# GitHub Actions (release.yml, windows job): install the pinned Microsoft Azure Artifact Signing
# client dlib (Microsoft.ArtifactSigning.Client 1.0.151), locate x64 signtool.exe from the
# Windows SDK, and write metadata.json + a Tauri --config overlay setting bundle.windows.signCommand.
#
# Secrets (AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET) are never written to disk or
# passed on any command line: Azure.CodeSigning.Dlib.dll reads them from the build step's
# environment via DefaultAzureCredential (restricted to EnvironmentCredential via ExcludeCredentials).
set -euo pipefail

: "${AZURE_SIGNING_ENDPOINT:?}" "${AZURE_SIGNING_ACCOUNT:?}" "${AZURE_SIGNING_PROFILE:?}"
: "${RUNNER_TEMP:?}" "${GITHUB_ENV:?}" "${GITHUB_PATH:?}"

# Microsoft Artifact Signing only accepts HTTPS endpoints under *.codesigning.azure.net.
if [[ ! "$AZURE_SIGNING_ENDPOINT" =~ ^https://[a-zA-Z0-9.-]+\.codesigning\.azure\.net/?$ ]]; then
  echo "::error::windows-signing-setup: AZURE_SIGNING_ENDPOINT must be an https://*.codesigning.azure.net URL" >&2
  exit 1
fi

version=1.0.151
pkg_sha256=35375a4bc4e1102da729fc3d8d9925f774c3baae6b8b7ff6398d948ca9ab0d8a
pkg_url="https://api.nuget.org/v3-flatcontainer/microsoft.artifactsigning.client/${version}/microsoft.artifactsigning.client.${version}.nupkg"

dir="$RUNNER_TEMP/azure-signing"
rm -rf "$dir"
mkdir -p "$dir/client"

zip_path="$dir/Microsoft.ArtifactSigning.Client.${version}.zip"
curl -fsSL --retry 5 --retry-all-errors --retry-delay 5 --connect-timeout 30 -o "$zip_path.part" "$pkg_url"
mv "$zip_path.part" "$zip_path"

actual=$(sha256sum < "$zip_path")
if [[ "${actual%% *}" != "$pkg_sha256" ]]; then
  echo "::error::windows-signing-setup: SHA-256 mismatch for Microsoft.ArtifactSigning.Client ${version}: ${actual%% *}, pinned ${pkg_sha256}" >&2
  exit 1
fi
echo "windows-signing-setup: Microsoft.ArtifactSigning.Client ${version} SHA-256 matches the pin"

if command -v unzip >/dev/null 2>&1; then
  unzip -qo "$zip_path" -d "$dir/client"
else
  pwsh -NoProfile -Command "Expand-Archive -LiteralPath '$(cygpath -w "$zip_path")' -DestinationPath '$(cygpath -w "$dir/client")' -Force"
fi
rm -f "$zip_path"

dlib="$dir/client/bin/x64/Azure.CodeSigning.Dlib.dll"
[[ -f "$dlib" ]] || { echo "::error::windows-signing-setup: $dlib not found after extraction" >&2; exit 1; }

# Find the newest compatible x64 signtool.exe in Windows Kits 10 (skip 10.0.20348.*, unsupported by the dlib).
kits_bin="${WINDOWS_KITS_BIN:-/c/Program Files (x86)/Windows Kits/10/bin}"
signtool_unix=$(
  shopt -s nullglob
  for f in "$kits_bin"/10.0.*/x64/signtool.exe; do
    [[ "$f" != */10.0.20348.*/x64/signtool.exe ]] || continue
    printf '%s\n' "$f"
  done | sort -V | tail -n 1
)
[[ -n "$signtool_unix" && -f "$signtool_unix" ]] || {
  echo "::error::windows-signing-setup: no compatible x64 signtool.exe found under $kits_bin" >&2
  exit 1
}

to_win() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s\n' "$1"
  fi
}

signtool_dir_win=$(to_win "$(dirname "$signtool_unix")")
echo "$signtool_dir_win" >> "$GITHUB_PATH"
echo "windows-signing-setup: using signtool from $signtool_dir_win"

metadata="$dir/metadata.json"
jq -n \
  --arg endpoint "$AZURE_SIGNING_ENDPOINT" \
  --arg account "$AZURE_SIGNING_ACCOUNT" \
  --arg profile "$AZURE_SIGNING_PROFILE" \
  '{
    Endpoint: $endpoint,
    CodeSigningAccountName: $account,
    CertificateProfileName: $profile,
    ExcludeCredentials: [
      "ManagedIdentityCredential",
      "WorkloadIdentityCredential",
      "SharedTokenCacheCredential",
      "VisualStudioCredential",
      "VisualStudioCodeCredential",
      "AzureCliCredential",
      "AzurePowerShellCredential",
      "AzureDeveloperCliCredential",
      "InteractiveBrowserCredential"
    ]
  }' > "$metadata"

dlib_win=$(to_win "$dlib")
metadata_win=$(to_win "$metadata")
conf="$dir/tauri.windows-sign.conf.json"
jq -n \
  --arg dlib "$dlib_win" \
  --arg dmdf "$metadata_win" \
  '{
    bundle: {
      windows: {
        signCommand: {
          cmd: "signtool.exe",
          args: [
            "sign", "/v",
            "/fd", "SHA256",
            "/tr", "http://timestamp.acs.microsoft.com",
            "/td", "SHA256",
            "/dlib", $dlib,
            "/dmdf", $dmdf,
            "/d", "Dash Wallet",
            "%1"
          ]
        }
      }
    }
  }' > "$conf"

conf_win=$(to_win "$conf")
echo "WINDOWS_SIGN_CONFIG=$conf_win" >> "$GITHUB_ENV"
echo "windows-signing-setup: wrote signing config overlay to $conf_win"
