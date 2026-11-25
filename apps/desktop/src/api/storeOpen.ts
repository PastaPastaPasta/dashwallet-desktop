// The store-refusal, StorageInUse and restore shapes (`storage_restore`, DTO contract v3) and
// `NetworkInfo.restored` come from dw-app's ts-rs export (src/api/generated). This module adds the runtime
// checks an open's error goes through before the screen trusts it.

import type { NetworkInfo } from "./generated/NetworkInfo";

import type { ErrorData } from "./generated/ErrorData";
import type { NotPrivateReason } from "./generated/NotPrivateReason";
import type { RestoreStateName } from "./generated/RestoreStateName";
import type { StoreButton } from "./generated/StoreButton";
import type { StoreHint } from "./generated/StoreHint";
import type { StoreLine } from "./generated/StoreLine";
import type { StoreScreen } from "./generated/StoreScreen";

export type { NotPrivateReason, StoreButton, StoreHint, StoreLine, StoreScreen };

/** `cause` is the engine's error variant, for tests and logs; the screen reads it only for a way out the
 * engine's screen doesn't carry yet (StoreRefusal's `LOCAL_WAYS_OUT`). */
export type StoreRefusedData = Extract<ErrorData, { kind: "storeRefused" }>;

export type StorageInUseData = Extract<ErrorData, { kind: "storageInUse" }>;

/** `storage_not_private`'s data: the directory others can change (the data root or one above it) and why. */
export type NotPrivateData = Extract<ErrorData, { kind: "notPrivate" }>;

export const NOT_PRIVATE_REASONS = ["groupWritable", "otherWritable", "otherOwner"] as const satisfies readonly NotPrivateReason[];

/** Where a pending restore stopped the open (`run_pending_restore`); the request, on the next start. */
export type RestoreState = RestoreStateName;

/** The causes the engine names, by state; `noSpace` has none of its own. */
export const RESTORE_CAUSES = {
  refused: ["missingBackup", "mismatchedBackup", "garbledMarker"],
  noSpace: ["diskFull"],
  notStarted: ["diskFull", "permissionDenied", "fileVanished", "other"],
  incomplete: ["diskFull", "permissionDenied", "fileVanished", "other"],
  foreign: ["otherLineage", "oldBuildWrote"],
} as const satisfies Record<RestoreState, readonly string[]>;

const RESTORE_STATES = Object.keys(RESTORE_CAUSES) as RestoreState[];

/** `storage_restore`'s data. `cause` is for tests and logs: each state's screen is the same for every
 * cause (MG (n), (q)). `neededBytes` is the space the copies need, `noSpace` only. */
export type StoreRestoreData = Extract<ErrorData, { kind: "restore" }>;

/** `storage_retry`'s data: the open's store upgrade stopped on the machine (the disk full or not writable,
 * an I/O fault) and runs again at the next start. `folder` is the network's data folder. */
export type StoreRetryData = Extract<ErrorData, { kind: "retry" }>;

/** The open's refusals: a store refusal, StorageInUse with its `retryable`, a pending restore that
 * stopped the open, an upgrade that stopped on the machine, or a data folder others can change. */
export type StoreOpenError =
  | { code: "storage_refused"; message: string; data: StoreRefusedData }
  | { code: "storage_in_use"; message: string; data: StorageInUseData | null }
  | { code: "storage_restore"; message: string; data: StoreRestoreData }
  | { code: "storage_retry"; message: string; data: StoreRetryData }
  | { code: "storage_not_private"; message: string; data: NotPrivateData };

const object = (e: unknown): e is { code?: unknown; data?: unknown } => typeof e === "object" && e !== null;

/** Whether `e` is a `storage_refused`, whatever its data. */
export const isStoreRefusedCode = (e: unknown): e is { code: "storage_refused"; data?: unknown } => object(e) && e.code === "storage_refused";

const isText = (v: unknown): v is string => typeof v === "string";
const isButton = (b: unknown): b is StoreButton =>
  object(b) && isText((b as StoreButton).key) && isText((b as StoreButton).label) && typeof (b as StoreButton).destructive === "boolean" && typeof (b as StoreButton).defaultFocus === "boolean";
const isHint = (h: unknown): h is StoreHint => object(h) && isText((h as StoreHint).key) && isText((h as StoreHint).text);
const isLine: (l: unknown) => l is StoreLine = isHint;

function isScreen(s: unknown): s is StoreScreen {
  if (!object(s)) return false;
  const v = s as Partial<StoreScreen>;
  return isText(v.titleKey) && isText(v.title) && isText(v.textKey) && isText(v.text) && Array.isArray(v.lines) && v.lines.length > 0 && v.lines.every(isLine) && Array.isArray(v.buttons) && v.buttons.every(isButton) && (v.hint === null || isHint(v.hint));
}

/** The refusal `e` carries, when it is `storage_refused` with a whole screen; a malformed one is `null`. */
export function storeRefusal(e: unknown): StoreRefusedData | null {
  if (!isStoreRefusedCode(e) || !object(e.data)) return null;
  const data = e.data as Partial<StoreRefusedData>;
  return data.kind === "storeRefused" && isText(data.folder) && isScreen(data.screen) ? (data as StoreRefusedData) : null;
}

/** Whether `e` is a `storage_not_private`, whatever its data. */
export const isNotPrivateCode = (e: unknown): e is { code: "storage_not_private"; data?: unknown } => object(e) && e.code === "storage_not_private";

/** The folder and reason `e` carries, when it is `storage_not_private` with whole data; otherwise `null`. */
export function storeNotPrivate(e: unknown): NotPrivateData | null {
  if (!isNotPrivateCode(e) || !object(e.data)) return null;
  const d = e.data as Partial<NotPrivateData>;
  return d.kind === "notPrivate" && isText(d.folder) && NOT_PRIVATE_REASONS.includes(d.reason as NotPrivateReason) ? (d as NotPrivateData) : null;
}

/** `storage_in_use`'s `retryable`; `null` for any other error. Without the data (a dw-app before it)
 * the store is read as held until a restart. */
export function storageInUse(e: unknown): { retryable: boolean } | null {
  if (!object(e) || e.code !== "storage_in_use") return null;
  const data = e.data as Partial<StorageInUseData> | null | undefined;
  return { retryable: data?.kind === "storageInUse" && data.retryable === true };
}

/** Whether `e` is a `storage_restore`, whatever its data. */
export const isStoreRestoreCode = (e: unknown): e is { code: "storage_restore"; data?: unknown } => object(e) && e.code === "storage_restore";

/** The restore failure `e` carries, when it is `storage_restore` with whole data; a malformed one is `null`. */
export function storeRestore(e: unknown): StoreRestoreData | null {
  if (!isStoreRestoreCode(e) || !object(e.data)) return null;
  const d = e.data as Partial<StoreRestoreData>;
  const neededBytes = d.neededBytes ?? null;
  const bytes = neededBytes === null || (typeof neededBytes === "number" && Number.isSafeInteger(neededBytes) && neededBytes >= 0);
  return d.kind === "restore" && RESTORE_STATES.includes(d.state as RestoreState) && isText(d.cause) && isText(d.folder) && bytes ? { ...(d as StoreRestoreData), neededBytes } : null;
}

/** Whether `e` is a `storage_retry`, whatever its data. */
export const isStoreRetryCode = (e: unknown): e is { code: "storage_retry"; data?: unknown } => object(e) && e.code === "storage_retry";

/** The upgrade stop `e` carries, when it is `storage_retry` with whole data; a malformed one is `null`. */
export function storeRetry(e: unknown): StoreRetryData | null {
  if (!isStoreRetryCode(e) || !object(e.data)) return null;
  const d = e.data as Partial<StoreRetryData>;
  return d.kind === "retry" && isText(d.folder) ? (d as StoreRetryData) : null;
}

/** The network as `open_network` and `active_network` give it, with whether its wallet data was restored
 * and the engine has not yet cleared RESTORED (its first full sync). */
export type RestoredNetworkInfo = NetworkInfo & Required<Pick<NetworkInfo, "restored">>;

/** `restored` of a network: `undefined` (not known yet) when the network isn't an object or has no
 * boolean `restored`, so nothing reads a missing field as "not restored". */
export const isRestored = (network: unknown): boolean | undefined => {
  const restored = object(network) ? (network as Partial<NetworkInfo>).restored : undefined;
  return typeof restored === "boolean" ? restored : undefined;
};
