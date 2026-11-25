// The store-refusal and StorageInUse shapes come from dw-app's ts-rs export (src/api/generated). The
// restore shapes (`storage_restore`, `Restore`, `NetworkInfo.restored`; DTO contract v3) are hand-written
// until the bindings carry them. This module adds the runtime checks an open's error goes through before
// the screen trusts it.

import type { NetworkInfo } from "./generated/NetworkInfo";

import type { ErrorData } from "./generated/ErrorData";
import type { NotPrivateReason } from "./generated/NotPrivateReason";
import type { StoreButton } from "./generated/StoreButton";
import type { StoreHint } from "./generated/StoreHint";
import type { StoreScreen } from "./generated/StoreScreen";

export type { NotPrivateReason, StoreButton, StoreHint, StoreScreen };

/** `cause` is the engine's error variant, for tests and logs only: the screen never depends on it. */
export type StoreRefusedData = Extract<ErrorData, { kind: "storeRefused" }>;

export type StorageInUseData = Extract<ErrorData, { kind: "storageInUse" }>;

/** `storage_not_private`'s data: the directory others can change (the data root or one above it) and why. */
export type NotPrivateData = Extract<ErrorData, { kind: "notPrivate" }>;

export const NOT_PRIVATE_REASONS = ["groupWritable", "otherWritable", "otherOwner"] as const satisfies readonly NotPrivateReason[];

/** Where a pending restore stopped the open (`run_pending_restore`); the request, on the next start. */
export type RestoreState = "refused" | "noSpace" | "notStarted" | "incomplete";

/** The causes the engine names, by state; `noSpace` has none of its own. */
export const RESTORE_CAUSES = {
  refused: ["missingBackup", "mismatchedBackup", "garbledMarker"],
  noSpace: ["diskFull"],
  notStarted: ["diskFull", "permissionDenied", "fileVanished", "other"],
  incomplete: ["diskFull", "permissionDenied", "fileVanished", "other"],
} as const satisfies Record<RestoreState, readonly string[]>;

const RESTORE_STATES = Object.keys(RESTORE_CAUSES) as RestoreState[];

/** `storage_restore`'s data. `cause` is for tests and logs: each state's screen is the same for every
 * cause (MG (n), (q)). `neededBytes` is the space the copies need, `noSpace` only. */
export type StoreRestoreData = { kind: "restore"; state: RestoreState; cause: string; neededBytes: number | null; folder: string };

/** The open's refusals: a store refusal, StorageInUse with its `retryable`, a pending restore that
 * stopped the open, or a data folder others can change. */
export type StoreOpenError =
  | { code: "storage_refused"; message: string; data: StoreRefusedData }
  | { code: "storage_in_use"; message: string; data: StorageInUseData | null }
  | { code: "storage_restore"; message: string; data: StoreRestoreData }
  | { code: "storage_not_private"; message: string; data: NotPrivateData };

const object = (e: unknown): e is { code?: unknown; data?: unknown } => typeof e === "object" && e !== null;

/** Whether `e` is a `storage_refused`, whatever its data. */
export const isStoreRefusedCode = (e: unknown): e is { code: "storage_refused"; data?: unknown } => object(e) && e.code === "storage_refused";

const isText = (v: unknown): v is string => typeof v === "string";
const isButton = (b: unknown): b is StoreButton =>
  object(b) && isText((b as StoreButton).key) && isText((b as StoreButton).label) && typeof (b as StoreButton).destructive === "boolean" && typeof (b as StoreButton).defaultFocus === "boolean";
const isHint = (h: unknown): h is StoreHint => object(h) && isText((h as StoreHint).key) && isText((h as StoreHint).text);

function isScreen(s: unknown): s is StoreScreen {
  if (!object(s)) return false;
  const v = s as Partial<StoreScreen>;
  return isText(v.titleKey) && isText(v.title) && isText(v.textKey) && isText(v.text) && Array.isArray(v.buttons) && v.buttons.every(isButton) && (v.hint === null || isHint(v.hint));
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

/** The network as `open_network` and `active_network` give it, with whether its wallet data was restored
 * and the engine has not yet cleared RESTORED (its first full sync). */
export type RestoredNetworkInfo = NetworkInfo & { restored: boolean };

/** `restored` of a network; `false` for a dw-app before it, which never restores. */
export const isRestored = (network: unknown): boolean => object(network) && (network as Partial<RestoredNetworkInfo>).restored === true;
