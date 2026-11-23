#!/usr/bin/env bash
# GitHub Actions: keep the .deb files ci/github/linux-host-deps.sh downloaded, and install them
# again without the network, checked on every use.
#
#   ci/github/apt-cache.sh save DIR     after a download: copy the .deb files and the Ubuntu
#                                       indexes they are listed in into DIR, then verify DIR
#   ci/github/apt-cache.sh verify DIR   check DIR's chain of trust; fail on any break
#
# The chain, checked from the image, never from the cache:
#   1. each DIR/lists/*_InRelease verifies with gpgv against the image's Ubuntu archive keyring
#      (/usr/share/keyrings/ubuntu-archive-keyring.gpg), and only its signed text is read; it must
#      name this image's Ubuntu release and a pocket the image's ubuntu.sources lists (release,
#      updates, security, backports: never proposed);
#   2. each DIR/lists/*_Packages has the SHA-256 that its signed InRelease gives it;
#   3. each DIR/archives/*.deb has a SHA-256 that one of those Packages indexes lists.
# A .deb, an index or a release file that breaks the chain fails `verify`, and the job with it.
set -euo pipefail

keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
sources=${DW_APT_SOURCES:-/etc/apt/sources.list.d/ubuntu.sources}
saving=
# shellcheck disable=SC2016 # the quotes are literal; the key expands
die() {
  if [[ -n "$saving" ]]; then
    echo "::warning::apt-cache: $*. The download is not cached." >&2
  else
    echo "::error::apt-cache: $*. Nothing was installed from the cache.${APT_CACHE_KEY:+ If the cache entry is the bad one, delete it (gh cache delete '$APT_CACHE_KEY') and rerun.}" >&2
  fi
  exit 1
}
sha() { sha256sum < "$1" | cut -d' ' -f1; }

verify() {
  local dir=$1 rel name prefix signed pk rest path want n_rel=0 n_pk=0 n_deb=0 deb codename suite suites
  local -A listed=()
  local tmp
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"; trap - RETURN' RETURN
  shopt -s nullglob
  [[ -r "$keyring" ]] || die "no Ubuntu archive keyring at $keyring"
  # shellcheck source=/dev/null
  codename=$(. /etc/os-release && echo "${VERSION_CODENAME:?}")
  [[ -r "$sources" ]] || die "no $sources to read the image's pockets from"
  suites=$(awk '/^Suites:/ { for (i = 2; i <= NF; i++) print $i }' "$sources")
  [[ -n "$suites" ]] || die "$sources lists no Suites"
  for rel in "$dir"/lists/*_InRelease; do
    name=${rel##*/}
    prefix=${name%InRelease}
    signed="$tmp/$name"
    if ! gpgv --quiet --keyring "$keyring" --output "$signed" "$rel" 2> "$tmp/gpgv"; then
      cat "$tmp/gpgv" >&2
      die "$name does not verify against $keyring"
    fi
    [[ "$(awk '/^Codename: / { print $2; exit }' "$signed")" == "$codename" ]] ||
      die "$name is not a release file of $codename, this image's Ubuntu release"
    suite=$(awk '/^Suite: / { print $2; exit }' "$signed")
    if [[ -z "$suite" ]] || ! grep -qxF -- "$suite" <<< "$suites"; then
      die "$name is for ${suite:-no suite}, not one of the pockets in $sources (${suites//$'\n'/ })"
    fi
    n_rel=$((n_rel + 1))
    for pk in "$dir/lists/$prefix"*_Packages; do
      rest=${pk##*/}
      rest=${rest#"$prefix"}
      path=${rest//_//}
      want=$(awk -v path="$path" '/^SHA256:/ { s = 1; next } /^[^ ]/ { s = 0 } s && $3 == path { print $1; exit }' "$signed")
      [[ -n "$want" ]] || die "${pk##*/}: $name lists no SHA-256 for $path"
      [[ "$(sha "$pk")" == "$want" ]] || die "${pk##*/} does not have the SHA-256 that $name gives $path"
      n_pk=$((n_pk + 1))
      while read -r h; do listed[$h]=1; done < <(awk '/^SHA256: / { print $2 }' "$pk")
    done
  done
  for pk in "$dir"/lists/*_Packages*; do
    [[ -n "${pk##*_Packages}" ]] && die "${pk##*/}: a compressed index cannot be checked against its release file"
  done
  ((n_rel > 0 && n_pk > 0)) || die "$dir has no signed release file with a Packages index"
  for deb in "$dir"/archives/*.deb; do
    [[ -n "${listed[$(sha "$deb")]:-}" ]] || die "${deb##*/} is in no signed Packages index"
    n_deb=$((n_deb + 1))
  done
  ((n_deb > 0)) || die "$dir has no .deb files"
  echo "apt-cache: $n_deb .deb files, $n_pk Packages indexes and $n_rel release files verify against $keyring"
}

save() {
  local dir=$1 lists=/var/lib/apt/lists archives=/var/cache/apt/archives deb pk name prefix rel
  shopt -s nullglob
  rm -rf "$dir"
  mkdir -p "$dir/lists" "$dir/archives"
  for deb in "$archives"/*.deb; do
    cp "$deb" "$dir/archives/"
    sha "$deb" >> "$dir/hashes"
  done
  # Only the indexes that list one of these .deb files, and the release files they belong to.
  for pk in "$lists"/*_Packages; do
    if grep -qxFf "$dir/hashes" <(awk '/^SHA256: / { print $2 }' "$pk"); then
      name=${pk##*/}
      for rel in "$lists"/*_InRelease; do
        prefix=${rel##*/}
        prefix=${prefix%InRelease}
        if [[ "$name" == "$prefix"* ]]; then
          cp "$pk" "$rel" "$dir/lists/"
        fi
      done
    fi
  done
  rm "$dir/hashes"
  saving=1
  verify "$dir"
  du -sh "$dir" | cut -f1 | sed 's/^/apt-cache: saved /'
}

case "${1:-}" in
  save | verify) "$1" "${2:?usage: $0 save|verify DIR}" ;;
  *) echo "usage: $0 save|verify DIR" >&2; exit 2 ;;
esac
