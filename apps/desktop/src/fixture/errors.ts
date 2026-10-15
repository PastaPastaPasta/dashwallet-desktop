// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// One `AppError` per `ErrorData` class, with a code dw-app really pairs it with
// (rust/crates/dw-app/src/error). The map is keyed by every `ErrorData["kind"]`, so a class
// added on the Rust side fails `tsc` here until it has a sample. `null` data (most codes) is
// the `none` class.

import type { AppError } from "../api/generated/AppError";
import type { ErrorData } from "../api/generated/ErrorData";

export type ErrorClass = ErrorData["kind"] | "none";

const W = "a1".repeat(32);
const TXID = "7e".repeat(32);

type Samples = { [K in ErrorData["kind"]]: { code: AppError["code"]; data: Extract<ErrorData, { kind: K }> } };

const SAMPLES: Samples = {
  retryAfter: { code: "vault.throttled", data: { kind: "retryAfter", secs: 90 } },
  wrongPassphrase: { code: "vault.wrong_passphrase", data: { kind: "wrongPassphrase", failedAttempts: 3, retryAfterSecs: 60 } },
  rpc: { code: "console.rpc_error", data: { kind: "rpc", rpcCode: -8, rpcMessage: "Block height out of range" } },
  authorize: {
    code: "console.authorization_required",
    data: { kind: "authorize", purpose: { kind: "spend", maxDuffs: 100_000_000 }, walletId: W },
  },
  duffs: { code: "send.amount_exceeds_balance", data: { kind: "duffs", needed: 5_000_000_000, available: 3_019_698_287 } },
  credits: { code: "platform.insufficient_credits", data: { kind: "credits", needed: "120000000", available: "4000000" } },
  identityCredits: {
    code: "insufficient_credits",
    data: { kind: "identityCredits", identity: "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF", needed: "120000000", available: "4000000" },
  },
  limit: { code: "vault.quick_unlock_limit_exceeded", data: { kind: "limit", duffs: 10_000_000 } },
  fee: { code: "send.amount_with_fee_exceeds_balance", data: { kind: "fee", fee: 374, available: 3_019_698_287 } },
  spendCap: { code: "send.grant_exceeded", data: { kind: "spendCap", maxDuffs: 100_000_000, outflow: 150_000_000 } },
  recipient: { code: "send.invalid_address", data: { kind: "recipient", index: 1 } },
  budget: { code: "platform.grant_exceeded", data: { kind: "budget", purpose: "credits", needed: "50000000", remaining: "1000000" } },
  needsGrant: { code: "platform.needs_grant", data: { kind: "needsGrant", purpose: "funding" } },
  revoked: { code: "platform.lease_revoked", data: { kind: "revoked", cause: "lock" } },
  artifact: { code: "platform.broadcast_unknown", data: { kind: "artifact", artifact: "3f9a1c27".repeat(8) } },
  txid: { code: "contact.payment_locked", data: { kind: "txid", txid: TXID } },
  draft: { code: "registration.in_progress", data: { kind: "draft", draft: "reg-1" } },
  draftWait: { code: "registration.funding_unknown_recent", data: { kind: "draftWait", draft: "reg-1", waitSecs: 600 } },
  usernameRules: { code: "name.invalid", data: { kind: "usernameRules", rules: ["minLength", "noEdgeHyphen"] } },
  ineligible: { code: "contact.ineligible", data: { kind: "ineligible", reason: "noDashPayKeys" } },
  keysMissing: { code: "identity.keys_missing", data: { kind: "keysMissing", purpose: "encryption" } },
  profileTooLong: { code: "profile.too_long", data: { kind: "profileTooLong", field: "displayName", length: 26, max: 25 } },
  feature: { code: "platform.feature_off", data: { kind: "feature", feature: "platform" } },
  call: { code: "platform.not_implemented", data: { kind: "call", call: "create_invitation" } },
  reason: { code: "send.broadcast_rejected", data: { kind: "reason", reason: "bad-txns-inputs-missingorspent" } },
  txAction: { code: "tx_action_refused", data: { kind: "txAction", refusal: "instantLocked" } },
  number: { code: "height_out_of_range", data: { kind: "number", value: 9_999_999 } },
  size: { code: "contact.details_too_long", data: { kind: "size", bytes: 4096, max: 2048 } },
  outpoint: { code: "outpoint_not_found", data: { kind: "outpoint", txid: TXID, vout: 1 } },
  wallet: { code: "backup.already_exists", data: { kind: "wallet", walletId: W } },
  keysNetwork: { code: "compat.network_mismatch", data: { kind: "keysNetwork", mainnet: true } },
  feeRate: { code: "psbt.fee_rate_too_high", data: { kind: "feeRate", duffsPerKb: 1_000_000 } },
  stuckFunding: {
    code: "stuck_funding",
    data: { kind: "stuckFunding", refusal: { kind: "spendUnsettled", txid: TXID }, retryLater: true },
  },
};

export const ERROR_CLASSES: readonly ErrorClass[] = ["none", ...(Object.keys(SAMPLES) as ErrorData["kind"][])];

export function isErrorClass(s: string): s is ErrorClass {
  return (ERROR_CLASSES as readonly string[]).includes(s);
}

/** A sample `AppError` of `cls`, as a command rejects with it. */
export function sampleError(cls: ErrorClass): AppError {
  if (cls === "none") return { code: "internal", message: "fixture: an internal error with no data", data: null };
  const { code, data } = SAMPLES[cls];
  return { code, message: `fixture: ${code} (${cls})`, data };
}
