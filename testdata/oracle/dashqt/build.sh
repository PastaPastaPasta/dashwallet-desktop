#!/usr/bin/env bash
# Builds mnshare_oracle (MG-04b M9): Dash Core's own src/qt/mnsharesession.cpp
# and src/qt/bitcoinunits.cpp, unmodified, linked with the tag's libraries and
# a static Qt 5 Core from a Dash Core depends prefix.
#
# Usage: build.sh <dash git checkout> <depends prefix> <work dir>
#   <depends prefix>: e.g. <dash>/depends/x86_64-pc-linux-gnu, built with Qt
#   (needs lib/libQt5Core.a and native/bin/moc). Read only.
#   <work dir>: the tag's sources go to <work dir>/core (`git archive`, so the
#   checkout's working tree is never read); the binary is <work dir>/mnshare_oracle.
# TAG overrides the tag (default v24.0.0-rc.3). Re-running reuses a configured tree.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
dash="$1"
dep="$(cd "$2" && pwd)"
work="$3"
tag="${TAG:-v24.0.0-rc.3}"
jobs="${JOBS:-16}"
core="$work/core"

mkdir -p "$work"
if [[ ! -d "$core" ]]; then
  mkdir "$core"
  git -C "$dash" archive "$tag" | tar -x -C "$core"
fi

cd "$core"
[[ -x configure ]] || ./autogen.sh
if [[ ! -f config.status ]]; then
  # Only libraries are made below; --with-utils is there because configure
  # wants at least one program.
  CONFIG_SITE="$dep/share/config.site" ./configure --disable-wallet --with-gui=no --disable-tests \
    --disable-bench --disable-fuzz-binary --without-daemon --with-utils --without-libs --disable-zmq \
    --disable-man --disable-ccache CXXFLAGS="-O1 -g0" CFLAGS="-O1 -g0"
fi
# providertx.cpp and specialtx.cpp are the node library's; only their objects
# are made, not the library.
crypto=(base sph ssse3 sse41 x86_aesni x86_shani avx2)
crypto_la=() crypto_a=()
for c in "${crypto[@]}"; do
  crypto_la+=("crypto/libbitcoin_crypto_$c.la")
  crypto_a+=("$core/src/crypto/.libs/libbitcoin_crypto_$c.a")
done
make -C src -j"$jobs" libbitcoin_common.a libbitcoin_util.a libbitcoin_consensus.a libunivalue.la \
  "${crypto_la[@]}" secp256k1/libsecp256k1.la dashbls/libdashbls.la \
  evo/libbitcoin_node_a-providertx.o evo/libbitcoin_node_a-specialtx.o

src="$core/src"
"$dep/native/bin/moc" "$src/qt/bitcoinunits.h" -o "$work/moc_bitcoinunits.cpp"
cxx="$(sed -n 's/^CXX = //p' src/Makefile)"
includes=(-DHAVE_CONFIG_H -DQT_CORE_LIB -DQT_NO_KEYWORDS -I"$src" -I"$src/config" -I"$src/univalue/include"
  -I"$src/secp256k1/include" -isystem"$src/dashbls/include" -isystem"$src/dashbls/depends/relic/include"
  -isystem"$src/dashbls/depends/minialloc/include" -isystem"$src/immer" -isystem"$dep/include"
  -isystem"$dep/include/QtCore")
# shellcheck disable=SC2086 # CXX carries its flags (e.g. -std=c++20)
$cxx -O1 -g0 -pthread -no-pie "${includes[@]}" \
  "$here/mnshare_oracle.cpp" "$src/qt/mnsharesession.cpp" "$src/qt/bitcoinunits.cpp" "$work/moc_bitcoinunits.cpp" \
  "$src/evo/libbitcoin_node_a-providertx.o" "$src/evo/libbitcoin_node_a-specialtx.o" \
  "$src/libbitcoin_common.a" "$src/libbitcoin_consensus.a" "$src/libbitcoin_util.a" \
  "${crypto_a[@]}" \
  "$src/.libs/libunivalue.a" "$src/secp256k1/.libs/libsecp256k1.a" "$src/dashbls/.libs/libdashbls.a" \
  "$dep/lib/libgmp.a" "$dep/lib/libbacktrace.a" "$dep/lib/libQt5Core.a" "$dep/lib/libqtpcre2.a" -ldl \
  -o "$work/mnshare_oracle"
echo "built $work/mnshare_oracle"
