// Hand-written, not generated: the store-refusal and StorageInUse shapes dw-app's ts-rs export carries
// once its `ErrorData` gains `StoreRefused` and `StorageInUse { retryable }` and `EngineCode` gains
// `storage_refused` (DTO contract v2). They match what ts-rs writes (tag `kind`, camelCase fields). When
// the regenerated bindings have them, these become re-exports of those.

/** A button of a refused open's screen: its catalog key and English label. */
export type StoreButton = { key: string; label: string; destructive: boolean; defaultFocus: boolean };

/** The line under the buttons: its catalog key and English text. */
export type StoreHint = { key: string; text: string };

/** The screen the engine gives for a refused open (`screen_for`), in English, `{folder}` filled in. */
export type StoreScreen = { titleKey: string; title: string; textKey: string; text: string; buttons: Array<StoreButton>; hint: StoreHint | null };

/** `cause` is the engine's error variant, for tests and logs only: the screen never depends on it. */
export type StoreRefusedData = { kind: "storeRefused"; screen: StoreScreen; folder: string; cause: string };

export type StorageInUseData = { kind: "storageInUse"; retryable: boolean };

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
 * the store is held until a restart, as at this pin. */
export function storageInUse(e: unknown): { retryable: boolean } | null {
  if (!object(e) || e.code !== "storage_in_use") return null;
  const data = e.data as Partial<StorageInUseData> | null | undefined;
  return { retryable: data?.kind === "storageInUse" && data.retryable === true };
}
