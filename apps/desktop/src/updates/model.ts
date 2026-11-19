// The update check (PK-03a) as the screens word it: notify only, off by default (D-N1).

import type { UpdatesView } from "../api/generated/UpdatesView";

/** About's Updates line. */
export function updateText(v: UpdatesView): string {
  if (!v.supported) return "Updates come through Flatpak";
  switch (v.state.kind) {
    case "off":
      return "Not checked: the update check is off";
    case "notChecked":
      return "Checking for updates…";
    case "current":
      return "Dash Wallet is up to date";
    case "available":
      return `Version ${v.state.release.version} is available`;
    case "failed":
      return "Could not check for updates";
  }
}

/** The version the status bar's chip and About's link name, when an update is available. */
export function availableVersion(v: UpdatesView | undefined): string | null {
  return v?.state.kind === "available" ? v.state.release.version : null;
}

/** A check is due at launch: on, supported, and none ran since the app started. */
export function checkDue(v: UpdatesView | undefined): boolean {
  return !!v && v.supported && v.enabled && v.state.kind === "notChecked";
}
