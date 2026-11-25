// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// One `AppError` per `ErrorData` class, with a code dw-app really pairs it with
// (rust/crates/dw-app/src/error). The map is keyed by every `ErrorData["kind"]`, so a class
// added on the Rust side fails `tsc` here until it has a sample. `null` data (most codes) is
// the `none` class.

import type { AppError } from "../api/generated/AppError";
import type { ErrorData } from "../api/generated/ErrorData";
import { RESTORE_CAUSES, type RestoreState, type StoreOpenError, type StoreRefusedData, type StoreRestoreData } from "../api/storeOpen";

export type ErrorClass = ErrorData["kind"] | "none";

const W = "a1".repeat(32);
const TXID = "7e".repeat(32);

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
  credits: { code: "platform.insufficient_credits", data: { kind: "credits", needed: "120000000", available: "4000000" } },
  creditRange: { code: "credits.withdrawal_out_of_range", data: { kind: "creditRange", min: "1190000", max: "50000000000000" } },
  creditMinimum: { code: "credits.send_out_of_range", data: { kind: "creditMinimum", min: "500000" } },
  networkBusy: {
    code: "app.network_busy",
    data: { kind: "networkBusy", flows: ["registration"], stalled: false, mixing: false, broadcast: false, draining: false },
  },
  networkCloseRefused: { code: "wallet", data: { kind: "networkCloseRefused", reason: "walletOpening" } },
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
  storeRefused: {
    code: "storage_refused",
    data: {
      kind: "storeRefused",
      screen: {
        titleKey: "storeUpgrade.refused.title",
        title: "Dash Wallet can't open this wallet data safely",
        textKey: "storeUpgrade.refused.wayOut.previous",
        text: "This version of Dash Wallet can't safely open the wallet data in /data/testnet: it holds contact data that this version can't read yet. Dash Wallet didn't change anything in it. Keep /data/testnet as it is: it may hold the only record of recent payments, and support may need it. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
        lines: [
          { key: "storeUpgrade.refused.intro", text: "This version of Dash Wallet can't safely open the wallet data in /data/testnet" },
          { key: "storeUpgrade.refused.detail.contactsTable", text: "it holds contact data that this version can't read yet." },
          {
            key: "storeUpgrade.refused.unchanged",
            text: "Dash Wallet didn't change anything in it. Keep /data/testnet as it is: it may hold the only record of recent payments, and support may need it.",
          },
          {
            key: "storeUpgrade.refused.wayOut.previous",
            text: "Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
          },
        ],
        buttons: [{ key: "storeUpgrade.refused.quit", label: "Quit", destructive: false, defaultFocus: false }],
        hint: null,
      },
      folder: "/data/testnet",
      cause: "RetiredPoolWithoutAdopter",
    },
  },
  retry: { code: "storage_retry", data: { kind: "retry", folder: "/data/testnet" } },
  restore: { code: "storage_restore", data: { kind: "restore", state: "notStarted", cause: "permissionDenied", neededBytes: null, folder: "/data/testnet" } },
  storageInUse: { code: "storage_in_use", data: { kind: "storageInUse", retryable: true } },
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

/** dw-store-upgrade's copy (copy.rs), verbatim, by catalog key: the refusal family `screen_for` composes
 * ("{intro}: {detail} {unchanged} {wayOut}", MG's final cantOpen family), and `copy::restore_foreign`'s
 * screen, which the catalog has for `storage_restore`'s foreign state. */
export const STORE_COPY = {
  "storeUpgrade.refused.title": "Dash Wallet can't open this wallet data safely",
  "storeUpgrade.refused.intro": "This version of Dash Wallet can't safely open the wallet data in {folder}",
  "storeUpgrade.refused.unchanged":
    "Dash Wallet didn't change anything in it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.",
  "storeUpgrade.refused.detail.contactsTable": "it holds contact data that this version can't read yet.",
  "storeUpgrade.refused.detail.shapeMismatch": "its layout doesn't match the version it says it is, so another program or a test build may have changed it.",
  "storeUpgrade.refused.detail.devBuild": "it was written by a development build of Dash Wallet.",
  "storeUpgrade.refused.wayOut.previous":
    "Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
  "storeUpgrade.refused.wayOut.support": "Choose Report a problem.",
  "storeUpgrade.refused.wayOut.restore":
    "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. To go back to your wallet data from before the upgrade, choose Restore earlier data. Otherwise, choose Report a problem.",
  "storeUpgrade.refused.quit": "Quit",
  "storeUpgrade.refused.report": "Report a problem",
  "storeUpgrade.refused.restore": "Restore earlier data",
  "settings.about.links.issuesHint": "Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.",
  "storeUpgrade.restore.foreign.title": "These wallet files don't match the restore",
  "storeUpgrade.restore.foreign.text":
    "Dash Wallet was restoring your wallet data from before the upgrade, but the wallet files now in {folder} don't match that data, so it stopped without changing them. Another version of Dash Wallet or another program may have replaced them.",
  "storeUpgrade.restore.foreign.keep": "Keep the current files",
  "storeUpgrade.restore.foreign.keepMeaning":
    "This stops the restore and opens the files in {folder} as they are now. Nothing is deleted: the wallet data saved before the upgrade, and the copy taken when the restore started, stay in the upgrade-backups folder.",
  "storeUpgrade.restore.foreign.quit": "Quit",
} as const;

type CopyKey = keyof typeof STORE_COPY;

const button = (key: CopyKey, label: string = STORE_COPY[key]) => ({ key, label, destructive: false, defaultFocus: false });

type Detail = "contactsTable" | "shapeMismatch" | "devBuild";
type WayOut = "previous" | "support" | "restore";

/** `screen_for`'s screen for a refusal with `detail`, a file this start changed nothing in, and `wayOut`:
 * its buttons Restore earlier data (restore only), Report a problem (except previous) and Quit; the issues
 * hint with Report a problem. Its text key is its way out's. */
const refusedScreen = (folder: string, cause: string, detail: Detail, wayOut: WayOut): StoreRefusedData => {
  const line = (key: CopyKey) => STORE_COPY[key];
  const keys = ["storeUpgrade.refused.intro", `storeUpgrade.refused.detail.${detail}`, "storeUpgrade.refused.unchanged", `storeUpgrade.refused.wayOut.${wayOut}`] as const;
  const lines = keys.map((key) => ({ key, text: line(key).replaceAll("{folder}", folder) }));
  const text = `${line(keys[0])}: ${keys.slice(1).map(line).join(" ")}`;
  const reports = wayOut !== "previous";
  return {
    kind: "storeRefused",
    cause,
    folder,
    screen: {
      titleKey: "storeUpgrade.refused.title",
      title: line("storeUpgrade.refused.title"),
      textKey: `storeUpgrade.refused.wayOut.${wayOut}`,
      text: text.replaceAll("{folder}", folder),
      lines,
      buttons: [
        ...(wayOut === "restore" ? [button("storeUpgrade.refused.restore")] : []),
        ...(reports ? [button("storeUpgrade.refused.report")] : []),
        button("storeUpgrade.refused.quit"),
      ],
      hint: reports ? { key: "settings.about.links.issuesHint", text: line("settings.about.links.issuesHint") } : null,
    },
  };
};

/** `screen_for`'s screens, each with the variant that gives it: a fork file this build can't read yet, for
 * the version before (Quit only); one whose layout is off (Report a problem); and a migrated file with a
 * verified backup (Restore earlier data). */
export const STORE_SCREENS = {
  refusedPrevious: (folder: string) => refusedScreen(folder, "RetiredPoolWithoutAdopter", "contactsTable", "previous"),
  refusedSupport: (folder: string) => refusedScreen(folder, "ForkStoreShapeMismatch", "shapeMismatch", "support"),
  refusedRestore: (folder: string) => refusedScreen(folder, "PreHistoryFork", "devBuild", "restore"),
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
