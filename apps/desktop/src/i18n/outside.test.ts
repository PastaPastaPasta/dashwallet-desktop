// The copy that lives outside the screen files (MP-14): its English reads as before, and it follows
// the language in use.

import { describe, expect, it } from "vitest";
import { orUnavailable } from "../api/pending";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { wipeOpenText } from "../app/shellBridge";
import { SHIELDED_SYNC_COPY } from "../shielded/syncing";
import { t } from ".";

const english = () => [
  SHIELDED_SYNC_COPY.syncing,
  SHIELDED_SYNC_COPY.scanning,
  SHIELDED_SYNC_COPY.lastChecked("3 hours ago"),
  wipeOpenText(),
  t("app.link.failed", { error: "fixture: no" }),
  t("app.lockFailed", { error: "fixture: no" }),
  t("app.notYet.explore"),
];

describe("copy outside the screens", () => {
  it("reads as before in English", async () => {
    expect(english()).toEqual([
      "Shielded is still syncing. Try again when it finishes.",
      "Scanning…",
      "Last checked 3 hours ago",
      "The link was not opened: close “Delete all wallet data?” first",
      "Could not open the link: fixture: no",
      "Could not lock: fixture: no",
      "Explore is not available yet.",
    ]);
    const absent = Promise.reject("command take_links not found");
    await expect(orUnavailable(absent)).rejects.toThrow(/^This is not available in this build\.$/);
  });

  it("follows the language in use", () => {
    ui.set({ prefs: { ...DEFAULT_PREFS, language: "en-XA" } });
    try {
      for (const text of english()) expect(text).toMatch(/^⟦.+⟧$/);
    } finally {
      ui.set({ prefs: { ...DEFAULT_PREFS, language: null } });
    }
  });
});
