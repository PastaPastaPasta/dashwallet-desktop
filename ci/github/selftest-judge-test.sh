#!/usr/bin/env bash
# Tests ci/github/selftest-judge.jq: a complete report passes, and a report missing any required
# screen, or with any other fault the judge looks for, fails. Run by ci.yml (desktop-renderer).
#
#   ci/github/selftest-judge-test.sh
set -euo pipefail

judge="$(cd "$(dirname "$0")" && pwd)/selftest-judge.jq"
failures=0

check() { # name, check: one passing check record
  jq -n --arg name "$1" '{id: $name, spec: "U3", selector: ".x", property: "color", expected: "red", actual: "red", pass: true}'
}

# A report as a funded, encrypted regtest wallet produces it.
good=$(jq -n --argjson ok "$(check ok)" '
  {
    final: true,
    screens: ([("light", "dark") as $t | ("overview", "transactions", "txDetail", "unlock") as $s
      | {key: "\($t)/\($s)", value: {boxes: [], checks: [$ok]}}] | from_entries),
    shown: {heroAmount: "1.50", heroFull: "1.50000000 tDASH", rows: [{txid: "ab", recordIndex: 0, amount: "+1.50"}]},
    security: {unlistedCommand: "refused: denied", navigation: "refused: still on the app origin"}
  }')

expect() { # name, jq edit of the good report, problem text expected ("" = passes)
  local out
  out=$(jq -r -f "$judge" <<<"$(jq "$2" <<<"$good")")
  if [[ -z "$3" ]]; then
    [[ -z "$out" ]] && return
    echo "FAIL $1: expected no problem, got: $out"
  else
    grep -qF -- "$3" <<<"$out" && return
    echo "FAIL $1: expected \"$3\", got: ${out:-nothing}"
  fi
  failures=$((failures + 1))
}

expect "complete report" '.' ""
for theme in light dark; do
  for screen in overview transactions txDetail unlock; do
    expect "no $theme/$screen" "del(.screens[\"$theme/$screen\"])" "screen $theme/$screen was not recorded"
  done
done
expect "no screens at all" 'del(.screens)' "screen light/overview was not recorded"
expect "checkpoint report" '.final = false' "the report is a checkpoint"
expect "no history rows" '.shown.rows = []' "showed no history row"
expect "nothing shown" 'del(.shown)' "showed no history row"
expect "a failed check" '.screens["dark/txDetail"].checks[0] |= (.pass = false | .actual = "blue")' \
  "dark/txDetail: ok (.x color) is blue, expected red"
expect "a history-row check without its element" \
  '.screens["light/transactions"].checks += [{id: "row", selector: ".tx-row-amount", property: "color", expected: "red", actual: "missing", pass: false}]' \
  "light/transactions: row (.tx-row-amount color) is missing"
expect "a hero-breakdown check without its element" \
  '.screens["light/overview"].checks += [{id: "bd", selector: "[data-testid=\"hero-breakdown\"]", property: "color", expected: "red", actual: "missing", pass: false}]' \
  "light/overview: bd"
expect "a check not applicable to the data" '.screens["dark/overview"].checks[0].pass = null' ""
expect "an unrefused probe" '.security.windowOpen = "NOT REFUSED: window.open returned a window"' "security windowOpen: NOT REFUSED"
expect "no navigation probe" 'del(.security.navigation)' "no navigation probe"

if ((failures)); then
  echo "selftest-judge: $failures case(s) failed"
  exit 1
fi
echo "selftest-judge: all cases pass"
