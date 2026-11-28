// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// One `AppError` per `ErrorData` class, with a code dw-app really pairs it with
// (rust/crates/dw-app/src/error). The map is keyed by every `ErrorData["kind"]`, so a class
// added on the Rust side fails `tsc` here until it has a sample. `null` data (most codes) is
// the `none` class.

import type { AppError } from "../api/generated/AppError";
import type { ErrorData } from "../api/generated/ErrorData";
import type { StoreLine } from "../api/generated/StoreLine";
import { storeUpgrade } from "../i18n/catalog/storeUpgrade";
import { en } from "../i18n/en";
import { RESTORE_CAUSES, type NotPrivateData, type NotPrivateReason, type RestoreState, type StoreOpenError, type StoreRefusedData, type StoreRestoreData } from "../api/storeOpen";

export type ErrorClass = ErrorData["kind"] | "none";

const W = "a1".repeat(32);
const TXID = "7e".repeat(32);

/** dw-store-upgrade's copy (copy.rs) by catalog key, read from the catalog so a production bundle holds
 * no fixture literal (check-bundle); StoreRefusal.test.ts holds it to copy.rs verbatim. The refusal family `screen_for` composes
 * ("{intro}: {detail} {unchanged} {wayOut}", MG's final cantOpen family), and `copy::restore_foreign`'s
 * screen, which the catalog has for `storage_restore`'s foreign state. */
const key = (...parts: string[]) => parts.join(".");
const refused = (...parts: string[]) => key("storeUpgrade", "refused", ...parts);
const ISSUES_HINT = key("settings", "about", "links", "issuesHint");

const COPY_KEYS = [
  ...["title", "intro", "unchanged"].map((name) => refused(name)),
  ...["contactsTable", "shapeMismatch", "devBuild", "damaged", "historyMismatch"].map((name) => refused("detail", name)),
  ...["previous", "support", "damaged", "restore", "retired"].map((name) => refused("wayOut", name)),
  ...["quit", "report", "restore", "recover"].map((name) => refused(name)),
  ISSUES_HINT,
  ...["title", "text", "keep", "keepMeaning", "quit"].map((name) => key("storeUpgrade", "restore", "foreign", name)),
];

const english = (copyKey: string): string => {
  const text = copyKey.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], en);
  if (typeof text !== "string") throw new Error(`fixture: no catalog text for ${copyKey}`);
  return text;
};

export const STORE_COPY: Readonly<Record<string, string>> = Object.fromEntries(COPY_KEYS.map((copyKey) => [copyKey, english(copyKey)]));

const button = (name: string) => ({ key: refused(name), label: english(refused(name)), destructive: false, defaultFocus: false });

type Detail = "contactsTable" | "shapeMismatch" | "devBuild" | "damaged" | "historyMismatch";
type WayOut = "previous" | "support" | "restore" | "damaged" | "retired";

/** `screen_for`'s screen for a refusal with `detail`, a file this start changed nothing in, and `wayOut`:
 * its buttons Set up from recovery phrase (damaged or retired), Restore earlier data (restore only), Report a
 * problem (except previous) and Quit; the issues
 * hint with Report a problem. Its text key is its way out's. */
const refusedScreen = (folder: string, cause: string, detail: Detail, wayOut: WayOut): StoreRefusedData => {
  const keys = [refused("intro"), refused("detail", detail), refused("unchanged"), refused("wayOut", wayOut)];
  const lines = keys.map((lineKey) => ({ key: lineKey, text: english(lineKey).replaceAll("{folder}", folder) }));
  const text = `${english(keys[0])}: ${keys.slice(1).map(english).join(" ")}`;
  const reports = wayOut !== "previous";
  return {
    kind: "storeRefused",
    cause,
    folder,
    screen: {
      titleKey: refused("title"),
      title: english(refused("title")),
      textKey: refused("wayOut", wayOut),
      text: text.replaceAll("{folder}", folder),
      lines,
      buttons: [
        ...(wayOut === "damaged" || wayOut === "retired" ? [button("recover")] : []),
        ...(wayOut === "restore" ? [button("restore")] : []),
        ...(reports ? [button("report")] : []),
        button("quit"),
      ],
      hint: reports ? { key: ISSUES_HINT, text: english(ISSUES_HINT) } : null,
    },
  };
};

type Samples = { [K in ErrorData["kind"]]: { code: AppError["code"]; data: Extract<ErrorData, { kind: K }> } };

const SAMPLES: Samples = {
  sharedMessage: { code: "masternodes.shared_message", data: { kind: "sharedMessage", reason: "bad_signature", share: 2 } },
  sharedStage: { code: "masternodes.shared_stage", data: { kind: "sharedStage", stage: "replied" } },
  sharedFunding: { code: "masternodes.shared_funding_mismatch", data: { kind: "sharedFunding", reason: "fee_low", needed: 120_000 } },
  sharedActorFee: { code: "masternodes.shared_actor_fee", data: { kind: "sharedActorFee", fee: 1_000 } },
  sharedSendNetwork: { code: "app.shared_send_off", data: { kind: "sharedSendNetwork", network: "mainnet" } },
  sharedUnsupported: { code: "app.shared_send_unsupported", data: { kind: "sharedUnsupported", sessionKind: "key_change" } },
  retryAfter: { code: "vault.throttled", data: { kind: "retryAfter", secs: 90 } },
  wrongPassphrase: { code: "vault.wrong_passphrase", data: { kind: "wrongPassphrase", failedAttempts: 3, retryAfterSecs: 60 } },
  rpc: { code: "console.rpc_error", data: { kind: "rpc", rpcCode: -8, rpcMessage: "Block height out of range" } },
  authorize: {
    code: "console.authorization_required",
    data: { kind: "authorize", purpose: { kind: "spend", maxDuffs: 100_000_000 }, walletId: W },
  },
  duffs: { code: "send.amount_exceeds_balance", data: { kind: "duffs", needed: 5_000_000_000, available: 3_019_698_287 } },
  connectFailure: { code: "connect.failed", data: { kind: "connectFailure", failure: "keyConflict" } },
  connectNetwork: { code: "connect.network", data: { kind: "connectNetwork", requested: "mainnet" } },
  credits: { code: "platform.insufficient_credits", data: { kind: "credits", needed: "120000000", available: "4000000" } },
  creditRange: { code: "credits.withdrawal_out_of_range", data: { kind: "creditRange", min: "1190000", max: "50000000000000" } },
  creditMinimum: { code: "credits.send_out_of_range", data: { kind: "creditMinimum", min: "500000" } },
  networkBusy: {
    code: "app.network_busy",
    data: { kind: "networkBusy", flows: ["registration"], stalled: false, mixing: false, broadcast: false, draining: false },
  },
  networkCloseRefused: { code: "wallet", data: { kind: "networkCloseRefused", reason: "walletOpening" } },
  shieldedHold: { code: "shielded_spend_unresolved", data: { kind: "shieldedHold", reason: "inFlight" } },
  updateBusy: { code: "app.update_busy", data: { kind: "updateBusy", reason: "payment" } },
  networkSwitch: {
    code: "app.network_switch_failed",
    data: { kind: "networkSwitch", network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" }, rolledBack: true },
  },
  identityCredits: {
    code: "insufficient_credits",
    data: { kind: "identityCredits", identity: "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF", needed: "120000000", available: "4000000" },
  },
  limit: { code: "vault.quick_unlock_limit_exceeded", data: { kind: "limit", duffs: 10_000_000 } },
  maxMinimum: { code: "credits.below_minimum", data: { kind: "maxMinimum", min: 56_001, duffs: 3_852, leftDuffs: 10_000_100, arrivingDuffs: 0, arrivingClears: false, arrivingBlock: null, otherArrivingDuffs: 0 } },
  fee: { code: "send.amount_with_fee_exceeds_balance", data: { kind: "fee", fee: 374, available: 3_019_698_287 } },
  spendCap: { code: "send.grant_exceeded", data: { kind: "spendCap", maxDuffs: 100_000_000, outflow: 150_000_000 } },
  recipient: { code: "send.invalid_address", data: { kind: "recipient", index: 1 } },
  budget: { code: "platform.grant_exceeded", data: { kind: "budget", purpose: "credits", needed: "50000000", remaining: "1000000" } },
  needsGrant: { code: "platform.needs_grant", data: { kind: "needsGrant", purpose: "funding" } },
  revoked: { code: "platform.lease_revoked", data: { kind: "revoked", cause: "lock" } },
  link: { code: "app.browser_open", data: { kind: "link", url: "https://www.dash.org" } },
  artifact: { code: "platform.broadcast_unknown", data: { kind: "artifact", artifact: "3f9a1c27".repeat(8) } },
  txid: { code: "contact.payment_locked", data: { kind: "txid", txid: TXID } },
  draft: { code: "registration.in_progress", data: { kind: "draft", draft: "reg-1" } },
  draftWait: { code: "registration.funding_unknown_recent", data: { kind: "draftWait", draft: "reg-1", waitSecs: 600 } },
  shieldedNotReady: { code: "registration.shielded_not_ready", data: { kind: "shieldedNotReady", reason: "rested", readyAt: 1_800_000_000 } },
  usernameRules: { code: "name.invalid", data: { kind: "usernameRules", rules: ["minLength", "noEdgeHyphen"] } },
  ineligible: { code: "contact.ineligible", data: { kind: "ineligible", reason: "noDashPayKeys" } },
  keysMissing: { code: "identity.keys_missing", data: { kind: "keysMissing", purpose: "encryption" } },
  profileTooLong: { code: "profile.too_long", data: { kind: "profileTooLong", field: "displayName", length: 26, max: 25 } },
  profileConflict: {
    code: "profile.conflict",
    data: {
      kind: "profileConflict",
      current: {
        displayName: "Alice",
        publicMessage: "Edited on another device",
        avatarUrl: null,
        avatarHash: null,
        avatarFingerprint: null,
        updatedAt: null,
      },
    },
  },
  feature: { code: "platform.feature_off", data: { kind: "feature", feature: "platform" } },
  governancePhase: { code: "governance.not_synced", data: { kind: "governancePhase", phase: "syncingVotes" } },
  proposalHash: { code: "governance.proposal_not_found", data: { kind: "proposalHash", hash: "5c".repeat(32) } },
  proposalFields: { code: "governance.invalid_proposal", data: { kind: "proposalFields", fields: ["name", "payment_address"] } },
  contestLabel: { code: "governance.contest_closed", data: { kind: "contestLabel", label: "a11ce" } },
  contender: { code: "governance.not_a_contender", data: { kind: "contender", identity: "8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P" } },
  call: { code: "platform.not_implemented", data: { kind: "call", call: "create_invitation" } },
  masternodeKey: { code: "masternodes.key_mismatch", data: { kind: "masternodeKey", role: "owner" } },
  unsupported: { code: "masternodes.unsupported", data: { kind: "unsupported", feature: "shared_masternodes", field: null } },
  reason: { code: "send.broadcast_rejected", data: { kind: "reason", reason: "bad-txns-inputs-missingorspent" } },
  txAction: { code: "tx_action_refused", data: { kind: "txAction", refusal: "instantLocked", walletId: null } },
  number: { code: "height_out_of_range", data: { kind: "number", value: 9_999_999 } },
  size: { code: "contact.details_too_long", data: { kind: "size", bytes: 4096, max: 2048 } },
  outpoint: { code: "outpoint_not_found", data: { kind: "outpoint", txid: TXID, vout: 1 } },
  wallet: { code: "backup.already_exists", data: { kind: "wallet", walletId: W } },
  keysNetwork: { code: "compat.network_mismatch", data: { kind: "keysNetwork", mainnet: true } },
  feeRate: { code: "psbt.fee_rate_too_high", data: { kind: "feeRate", duffsPerKb: 1_000_000 } },
  protocol: { code: "shielded.not_active", data: { kind: "protocol", protocolVersion: 11, activationVersion: 12 } },
  poolNotes: { code: "shielded.pool_too_small", data: { kind: "poolNotes", notes: 12, required: 250 } },
  stuckFunding: {
    code: "stuck_funding",
    data: { kind: "stuckFunding", refusal: { kind: "spendUnsettled", txid: TXID }, retryLater: true },
  },
  storeRefused: { code: "storage_refused", data: refusedScreen("/data/testnet", "RetiredPoolWithoutAdopter", "contactsTable", "previous") },
  retry: { code: "storage_retry", data: { kind: "retry", folder: "/data/testnet" } },
  restore: { code: "storage_restore", data: { kind: "restore", state: "notStarted", cause: "permissionDenied", neededBytes: null, folder: "/data/testnet" } },
  storageInUse: { code: "storage_in_use", data: { kind: "storageInUse", retryable: true } },
  notPrivate: { code: "storage_not_private", data: { kind: "notPrivate", folder: "/home/user/.local/share/dashwallet", reason: "groupWritable" } },
  io: { code: "io", data: { kind: "io", retryable: true } },
  operationState: { code: "masternodes.operation_state", data: { kind: "operationState", state: "inputs_spent" } },
  keyRole: { code: "masternodes.keys_exhausted", data: { kind: "keyRole", role: "voting" } },
  operation: { code: "masternodes.update_pending", data: { kind: "operation", operation: "op-1" } },
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

/** `screen_for`'s screens, each with the variant that gives it: a fork file this build can't read yet, for
 * the version before (Quit only); one whose layout is off (Report a problem); a migrated file with a
 * verified backup (Restore earlier data); and a damaged store or retired backup (Set up from recovery phrase). */
export const STORE_SCREENS = {
  refusedPrevious: (folder: string) => refusedScreen(folder, "RetiredPoolWithoutAdopter", "contactsTable", "previous"),
  refusedSupport: (folder: string) => refusedScreen(folder, "ForkStoreShapeMismatch", "shapeMismatch", "support"),
  refusedRestore: (folder: string) => refusedScreen(folder, "PreHistoryFork", "devBuild", "restore"),
  damaged: (folder: string) => refusedScreen(folder, "SnapshotSourceDamaged", "damaged", "damaged"),
  retiredBackup: (folder: string) => refusedScreen(folder, "SchemaHistoryMismatch", "historyMismatch", "retired"),
};

export type StoreScreenName = keyof typeof STORE_SCREENS;
export const STORE_SCREEN_NAMES = Object.keys(STORE_SCREENS) as StoreScreenName[];

/** `storage_refused` as `open_network` rejects with it. */
export const storeRefusedError = (data: StoreRefusedData): StoreOpenError => ({
  code: "storage_refused",
  message: `fixture: the wallet data in ${data.folder} was refused (${data.cause})`,
  data,
});

/** `storage_retry` as `open_network` rejects with it. */
export const storeRetryError = (folder: string): StoreOpenError => ({
  code: "storage_retry",
  message: `fixture: the upgrade of the wallet data in ${folder} stopped: disk full`,
  data: { kind: "retry", folder },
});

/** The store upgrade's notices `open_network` can carry (`copy::notice_line`), by name. */
export const UPGRADE_NOTICES = {
  snapshotDamaged: { key: "storeUpgrade.notice.snapshotDamaged", text: storeUpgrade.notice.snapshotDamaged },
  snapshotOther: { key: "storeUpgrade.notice.snapshotOther", text: storeUpgrade.notice.snapshotOther },
} as const satisfies Record<string, StoreLine>;
export type UpgradeNoticeName = keyof typeof UPGRADE_NOTICES;
export const UPGRADE_NOTICE_NAMES = Object.keys(UPGRADE_NOTICES) as UpgradeNoticeName[];

/** `storage_in_use` as `open_network` rejects with it. */
export const storageInUseError = (retryable: boolean, message = "fixture: the wallet data is still held by the session before"): StoreOpenError => ({
  code: "storage_in_use",
  message,
  data: { kind: "storageInUse", retryable },
});

/** Every `storage_restore` the fixture can give, `<state>:<cause>`. */
export const RESTORE_FAILURE_NAMES = (Object.keys(RESTORE_CAUSES) as RestoreState[]).flatMap((state) => RESTORE_CAUSES[state].map((cause) => `${state}:${cause}`));

/** The `storage_restore` data for `<state>:<cause>`; `noSpace` names the space its copies need. */
export function restoreFailure(name: string, folder: string): StoreRestoreData {
  const [state, cause] = name.split(":") as [RestoreState, string];
  return { kind: "restore", state, cause, neededBytes: state === "noSpace" ? 2_147_483_648 : null, folder };
}

/** `storage_restore` as `open_network` (or `request_restore`) rejects with it. */
export const storeRestoreError = (data: StoreRestoreData): StoreOpenError => ({
  code: "storage_restore",
  message: `fixture: restoring the wallet data in ${data.folder} stopped (${data.state}, ${data.cause})`,
  data,
});

/** The `storage_not_private` data for a data folder others can change, and why. */
export const notPrivateFailure = (reason: NotPrivateReason, folder: string): NotPrivateData => ({ kind: "notPrivate", folder, reason });

/** `storage_not_private` as `open_network` rejects with it. */
export const storeNotPrivateError = (data: NotPrivateData): StoreOpenError => ({
  code: "storage_not_private",
  message: `fixture: the data folder is not private: ${data.folder} (${data.reason})`,
  data,
});
