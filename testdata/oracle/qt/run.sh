#!/usr/bin/env bash
# Regenerates testdata/uri_cases.json and testdata/amount_format.json by
# running dash-qt's own URI and BitcoinUnits code against Qt 5.15.
# Needs Qt 5.15 (macOS: `brew install qt@5`; set QT5_PREFIX elsewhere).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
testdata="$(cd "$here/../.." && pwd)"
qt="${QT5_PREFIX:-/opt/homebrew/opt/qt@5}"
out="${TMPDIR:-/tmp}/dw-qt-oracle"
mkdir -p "$out"

python3 "$here/make_inputs.py"

if [[ "$(uname)" == "Darwin" ]]; then
  clang++ -std=c++17 -O1 -F"$qt/lib" -I"$qt/lib/QtCore.framework/Headers" \
    "$here/qt_oracle.cpp" -framework QtCore -Wl,-rpath,"$qt/lib" -o "$out/qt_oracle"
else
  g++ -std=c++17 -O1 -fPIC "$here/qt_oracle.cpp" $(pkg-config --cflags --libs Qt5Core) -o "$out/qt_oracle"
fi

"$out/qt_oracle" "$here/uri_inputs.json" "$testdata/uri_cases.json" \
  "$here/amount_inputs.json" "$testdata/amount_format.json"
python3 "$here/../jsonfmt.py" "$testdata/uri_cases.json" "$testdata/amount_format.json"
echo "wrote $testdata/uri_cases.json and $testdata/amount_format.json"
