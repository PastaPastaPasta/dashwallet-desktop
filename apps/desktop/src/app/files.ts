// Files the user picks in the OS's open and save dialogs (U-05 D2). dw-app shows the dialog and
// reads or writes the chosen file; the renderer never names a path. Screens call these, not the
// commands, so the encodings and the error texts are in one place.

import { errorMessage, hasCode, type Backend } from "../api/backend";
import type { FilePurpose } from "../api/generated/FilePurpose";
import type { OpenedFile } from "../api/generated/OpenedFile";

/** UTF-8 text as base64, for `saveFileAs`. */
export function textToBase64(text: string): string {
  return bytesToBase64(new TextEncoder().encode(text));
}

export function bytesToBase64(bytes: Uint8Array): string {
  let s = "";
  // In chunks: `String.fromCharCode(...bytes)` overflows the stack on large files.
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export function base64ToBytes(b64: string): Uint8Array {
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

/** The open dialog for `purpose`, then the chosen file; `null` when the user cancelled. */
export const openFile = (b: Pick<Backend, "openFile">, purpose: FilePurpose): Promise<OpenedFile | null> => b.openFile(purpose);

/** The save dialog, offering `name`, then `text` written there owner-only; the path, `null` when
 * the user cancelled. */
export const saveText = (b: Pick<Backend, "saveFileAs">, purpose: FilePurpose, name: string, text: string): Promise<string | null> =>
  b.saveFileAs(purpose, name, textToBase64(text));

/** `saveText` for bytes already in base64 (a PSBT). */
export const saveBase64 = (b: Pick<Backend, "saveFileAs">, purpose: FilePurpose, name: string, data: string): Promise<string | null> => b.saveFileAs(purpose, name, data);

/** The dialog alone, for a file the engine reads or writes itself (U-04). */
export const chooseFilePath = (b: Pick<Backend, "chooseFilePath">, purpose: FilePurpose, save: boolean, suggestedName: string | null): Promise<string | null> =>
  b.chooseFilePath(purpose, save, suggestedName);

/** Why a file call failed, for a line under the button. The too-large text is the caller's, which
 * knows what the file was (dash-qt: "PSBT file must be smaller than 100 MiB"). */
export function fileErrorText(e: unknown): string {
  if (hasCode(e, "app.file_dialog")) return "The file dialog could not be shown.";
  if (hasCode(e, "app.file_too_large")) return "The file is too large.";
  if (hasCode(e, "app.file_read")) return "The file could not be read.";
  if (hasCode(e, "app.file_write")) return "The file could not be saved.";
  return errorMessage(e);
}
