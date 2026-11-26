// The temporary files of Dash Core exports that did not finish, which the engine deletes at a
// session's open: `exportStagingDeleted` is a toast, `exportStagingNotDeleted` a banner on Home
// until dismissed (its path, for the user to delete). The engine may send them before the
// renderer listens, so the boot also reads them from dw-app's notice memory; each shows once per
// renderer, and a file still there comes back at the next start.

import { useSyncExternalStore } from "react";
import type { NoticeCodeName } from "../api/generated/NoticeCodeName";
import type { RecentNotice } from "../api/generated/RecentNotice";
import { showShellToast } from "../components/toast";
import { t } from "../i18n";

type StagingCode = Extract<NoticeCodeName, "exportStagingDeleted" | "exportStagingNotDeleted">;

const seen = new Set<string>();
let stuck: readonly string[] = [];
const listeners = new Set<() => void>();

export function isExportStagingNotice(code: NoticeCodeName): code is StagingCode {
  return code === "exportStagingDeleted" || code === "exportStagingNotDeleted";
}

export function onExportStagingNotice(code: StagingCode, path: string) {
  const key = `${code}\u0000${path}`;
  if (seen.has(key)) return;
  seen.add(key);
  if (code === "exportStagingDeleted") {
    showShellToast(t("home.exportStaging.deleted"));
    return;
  }
  stuck = [...stuck, path];
  listeners.forEach((l) => l());
}

/** The boot's read of `network`'s notices sent before the renderer listened. */
export async function readExportStagingNotices(recent: () => Promise<RecentNotice[]>, network: string) {
  for (const n of await recent()) {
    if (n.network === network && isExportStagingNotice(n.code)) onExportStagingNotice(n.code, n.detail);
  }
}

export function dismissExportStaging(path: string) {
  stuck = stuck.filter((p) => p !== path);
  listeners.forEach((l) => l());
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => void listeners.delete(l);
}

export function useStuckExportStaging(): readonly string[] {
  return useSyncExternalStore(subscribe, () => stuck);
}

/** `then` with the files still to show after each change. */
export function subscribeStuck(then: (paths: readonly string[]) => void): () => void {
  return subscribe(() => then(stuck));
}

/** Tests only. */
export function resetExportStaging() {
  seen.clear();
  stuck = [];
  listeners.forEach((l) => l());
}
