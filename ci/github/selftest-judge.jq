# Judges the report of the Tauri app's --selftest (apps/desktop/src/app/selftest.ts) in
# tauri-selftest.yml. Prints one line per problem; no output means the report passes.
#
#   jq -r -f ci/github/selftest-judge.jq selftest.json
#
# The app exits 0 whatever it found, so this is the verdict. The run must have recorded every
# gate screen in both appearances on a wallet with history (regtest-fund.py) and an encrypted
# vault the self-test locks and unlocks (--selftest-passphrase-file): Overview, Transactions,
# the transaction detail sheet and the lock screen. ci/github/selftest-judge-test.sh proves
# that a report missing any of them fails. No failed check is excused: a check of an element the
# data does not produce is "not applicable" (pass null) in the report itself.

def required_screens: [("light", "dark") as $theme | ("overview", "transactions", "txDetail", "unlock") as $screen | "\($theme)/\($screen)"];

. as $root |
(if .error != null then "the self-test failed: \(.error)" else empty end),
(if .final == false then "the report is a checkpoint: the self-test did not finish" else empty end),
(required_screens[] as $screen | select($root.screens[$screen] == null) | "screen \($screen) was not recorded"),
(if (.shown.rows // []) == [] then "the Transactions screen showed no history row" else empty end),
(.screens // {} | to_entries[] | .key as $screen | .value.checks[]
  | select(.pass == false)
  | "\($screen): \(.id) (\(.selector) \(.property)) is \(.actual), expected \(.expected)"),
(if .security.navigation == null then "no navigation probe: the self-test ended before it" else empty end),
(.security // {} | to_entries[] | select(.value | startswith("NOT REFUSED")) | "security \(.key): \(.value)")
