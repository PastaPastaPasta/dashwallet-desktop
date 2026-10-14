// The display settings dw-app keeps in `<data root>/settings.json` (U-04 D2), as the renderer
// holds them: in the ui store (`UiState.prefs`), read once at launch and saved through
// `set_ui_settings`. `--theme` wins over the saved appearance for the run (D3), and "Hide
// balances when the app starts" turns discreet mode on at launch (D14).

import type { Backend } from "../api/backend";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { UiSettings } from "../api/generated/UiSettings";
import { DEFAULT_PREFS, ui } from "../app/ui";

/** Reads the saved settings at launch; an engine that cannot answer leaves the defaults. */
export async function loadPrefs(backend: Backend, launch: LaunchInfo) {
  const prefs = await backend.uiSettings().catch(() => DEFAULT_PREFS);
  ui.set({ prefs, theme: launch.theme ?? prefs.theme, discreet: prefs.autohideBalance });
}

/** The settings the latest save asked for, until dw-app has answered it: a change made meanwhile
 * builds on it, not on the store, so it does not write the earlier change back. */
let wanted: UiSettings | null = null;
/** Saves are written one at a time, in the order they were made. */
let writes: Promise<unknown> = Promise.resolve();

/** Saves `patch` over the current settings; the store changes once dw-app has written them. */
export function savePrefs(backend: Backend, launch: LaunchInfo, patch: Partial<UiSettings>): Promise<UiSettings> {
  const next = { ...(wanted ?? ui.get().prefs), ...patch };
  wanted = next;
  const write = writes.then(() => backend.setUiSettings(next));
  writes = write.catch(() => undefined);
  const settled = () => {
    if (wanted === next) wanted = null;
  };
  return write.then(
    (prefs) => {
      settled();
      ui.set({ prefs, theme: launch.theme ?? prefs.theme });
      return prefs;
    },
    (e: unknown) => {
      settled();
      throw e;
    },
  );
}
