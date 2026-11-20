/** Why a self-test of a vault that opened locked, launched without its passphrase file, ends at
 * once (ci/github/selftest-judge.jq reports it as the run's failure). */
export const SELFTEST_LOCKED = "the vault is locked and the self-test has no way to unlock it: run it with --selftest-passphrase-file";
