// The store-refusal and StorageInUse shapes come from dw-app's ts-rs export (src/api/generated); this
// module adds the runtime checks a refused open's error goes through before the screen trusts it.

import type { ErrorData } from "./generated/ErrorData";
import type { StoreButton } from "./generated/StoreButton";
import type { StoreHint } from "./generated/StoreHint";
import type { StoreScreen } from "./generated/StoreScreen";

export type { StoreButton, StoreHint, StoreScreen };

/** `cause` is the engine's error variant, for tests and logs only: the screen never depends on it. */
export type StoreRefusedData = Extract<ErrorData, { kind: "storeRefused" }>;

export type StorageInUseData = Extract<ErrorData, { kind: "storageInUse" }>;

/** The open's refusals: a store refusal, or StorageInUse with its `retryable`. */
export type StoreOpenError = { code: "storage_refused"; message: string; data: StoreRefusedData } | { code: "storage_in_use"; message: string; data: StorageInUseData | null };

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

/** `storage_in_use`'s `retryable`; `null` for any other error. Without the data (a dw-app before it)
 * the store is read as held until a restart. */
export function storageInUse(e: unknown): { retryable: boolean } | null {
  if (!object(e) || e.code !== "storage_in_use") return null;
  const data = e.data as Partial<StorageInUseData> | null | undefined;
  return { retryable: data?.kind === "storageInUse" && data.retryable === true };
}
