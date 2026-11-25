// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// One `AppError` per `ErrorData` class, with a code dw-app really pairs it with
// (rust/crates/dw-app/src/error). The map is keyed by every `ErrorData["kind"]`, so a class
// added on the Rust side fails `tsc` here until it has a sample. `null` data (most codes) is
// the `none` class.

import type { AppError } from "../api/generated/AppError";
import type { ErrorData } from "../api/generated/ErrorData";
import { RESTORE_CAUSES, type NotPrivateData, type NotPrivateReason, type RestoreState, type StoreOpenError, type StoreRefusedData, type StoreRestoreData } from "../api/storeOpen";

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
        titleKey: "storeUpgrade.cantOpen.title",
        title: "Dash Wallet can't open this wallet data",
        textKey: "storeUpgrade.cantOpen.textWithoutBackup",
        text: "This version of Dash Wallet can't open the wallet data in /data/testnet. Nothing in it was changed.",
        buttons: [{ key: "storeUpgrade.cantOpen.quit", label: "Quit", destructive: false, defaultFocus: false }],
        hint: null,
      },
      folder: "/data/testnet",
      cause: "RetiredPoolWithoutAdopter",
    },
  },
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

/** dw-store-upgrade's copy (copy.rs), verbatim: what `screen_for` gives today, and the restore lane's
 * `copy::restore_foreign()` and wayOut.restore refusal (MG's cantOpen family, FINAL). */
export const STORE_COPY = {
  cantOpenTitle: "Dash Wallet can't open this wallet data",
  cantOpenText:
    "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed, and the wallet data saved before the upgrade is still in the upgrade-backups folder inside {folder}. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
  cantOpenTextWithoutBackup:
    "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",

  foreignTitle: "These wallet files don't match the restore",
  foreignText:
    "Dash Wallet was restoring your wallet data from before the upgrade, but the wallet files now in {folder} don't match that data, so it stopped without changing them. Another version of Dash Wallet or another program may have replaced them.",
  foreignKeep: "Keep the current files",
  foreignKeepMeaning:
    "This stops the restore and opens the files in {folder} as they are now. Nothing is deleted: the wallet data saved before the upgrade, and the copy taken when the restore started, stay in the upgrade-backups folder.",
  refusedTitle: "Dash Wallet can't open this wallet data safely",
  refusedRestoreText:
    "This version of Dash Wallet can't safely open the wallet data in {folder}: it was written by a development build of Dash Wallet. Dash Wallet didn't change anything in it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it. Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. To go back to your wallet data from before the upgrade, choose Restore earlier data. Otherwise, choose Report a problem.",
  issuesHint: "Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.",
};

const plain = (key: string, label: string) => ({ key, label, destructive: false, defaultFocus: false });

const cantOpenScreen = (folder: string, cause: string, text: "text" | "textWithoutBackup", copy: string): StoreRefusedData => ({
  kind: "storeRefused",
  cause,
  folder,
  screen: {
    titleKey: "storeUpgrade.cantOpen.title",
    title: STORE_COPY.cantOpenTitle,
    textKey: `storeUpgrade.cantOpen.${text}`,
    text: copy.replaceAll("{folder}", folder),
    buttons: [plain("storeUpgrade.cantOpen.quit", "Quit")],
    hint: null,
  },
});

/** The foreign lineage (`RestoreIncomplete { cause: S4OtherLineage }`): `copy::restore_foreign()`. */
const foreignScreen = (folder: string): StoreRefusedData => ({
  kind: "storeRefused",
  cause: "RestoreIncomplete",
  folder,
  screen: {
    titleKey: "storeUpgrade.restore.foreign.title",
    title: STORE_COPY.foreignTitle,
    textKey: "storeUpgrade.restore.foreign.text",
    text: STORE_COPY.foreignText.replaceAll("{folder}", folder),
    buttons: [plain("storeUpgrade.restore.foreign.keep", STORE_COPY.foreignKeep), plain("settings.about.links.issues", "Report a problem"), plain("storeUpgrade.restore.foreign.quit", "Quit")],
    hint: { key: "storeUpgrade.restore.foreign.keepMeaning", text: STORE_COPY.foreignKeepMeaning.replaceAll("{folder}", folder) },
  },
});

/** A refusal whose way out is `wayOut.restore` (migrated, backup Verified): Restore earlier data, Report
 * a problem and Quit. */
const refusedRestoreScreen = (folder: string): StoreRefusedData => ({
  kind: "storeRefused",
  cause: "PreHistoryFork",
  folder,
  screen: {
    titleKey: "storeUpgrade.refused.title",
    title: STORE_COPY.refusedTitle,
    textKey: "storeUpgrade.refused.wayOut.restore",
    text: STORE_COPY.refusedRestoreText.replaceAll("{folder}", folder),
    buttons: [plain("storeUpgrade.refused.restore", "Restore earlier data"), plain("storeUpgrade.refused.report", "Report a problem"), plain("storeUpgrade.refused.quit", "Quit")],
    hint: { key: "settings.about.links.issuesHint", text: STORE_COPY.issuesHint },
  },
});

/** `screen_for`'s screens, each with the variant that gives it: `cantOpen` with a verified backup, and
 * without one; the foreign lineage's; and a refusal that offers Restore earlier data. */
export const STORE_SCREENS = {
  cantOpen: (folder: string) => cantOpenScreen(folder, "RetiredPoolWithoutAdopter", "text", STORE_COPY.cantOpenText),
  cantOpenWithoutBackup: (folder: string) => cantOpenScreen(folder, "P9aBroken", "textWithoutBackup", STORE_COPY.cantOpenTextWithoutBackup),
  restoreForeign: foreignScreen,
  refusedRestore: refusedRestoreScreen,
};

export type StoreScreenName = keyof typeof STORE_SCREENS;
export const STORE_SCREEN_NAMES = Object.keys(STORE_SCREENS) as StoreScreenName[];

/** `storage_refused` as `open_network` rejects with it. */
export const storeRefusedError = (data: StoreRefusedData): StoreOpenError => ({
  code: "storage_refused",
  message: `fixture: the wallet data in ${data.folder} was refused (${data.cause})`,
  data,
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

/** The `storage_not_private` data for a data folder others can change, and why. */
export const notPrivateFailure = (reason: NotPrivateReason, folder: string): NotPrivateData => ({ kind: "notPrivate", folder, reason });

/** `storage_not_private` as `open_network` rejects with it. */
export const storeNotPrivateError = (data: NotPrivateData): StoreOpenError => ({
  code: "storage_not_private",
  message: `fixture: the data folder is not private: ${data.folder} (${data.reason})`,
  data,
});
