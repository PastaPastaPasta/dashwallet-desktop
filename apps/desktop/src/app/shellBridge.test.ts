// The wipe sheet holds the shell while it is open (U-04 D38).

import { afterEach, describe, expect, it, vi } from "vitest";
import { applyLink, runCommand } from "./shellBridge";
import { ui } from "./ui";

const initial = ui.get();

afterEach(() => {
  ui.set(initial);
  vi.unstubAllGlobals();
});

describe("while the wipe sheet is open", () => {
  it("IOS109_aMenuCommandLeavesThePageInPlace", () => {
    vi.stubGlobal("requestAnimationFrame", () => 0);
    ui.set({ route: "settings", wiping: true });
    const lock = vi.fn();
    runCommand({ kind: "navigate", route: "pay" }, lock);
    runCommand({ kind: "tools", tab: "console" }, lock);
    runCommand({ kind: "addWallet" }, lock);
    runCommand({ kind: "about" }, lock);
    expect(ui.get()).toMatchObject({ route: "settings", onboarding: false, dialog: null });
    // Locking draws over the sheet and keeps it.
    runCommand({ kind: "lock" }, lock);
    expect(lock).toHaveBeenCalledOnce();
    ui.set({ wiping: false });
    runCommand({ kind: "navigate", route: "pay" }, lock);
    expect(ui.get().route).toBe("pay");
  });

  it("IOS109_aLinkIsNotOpened", () => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    ui.set({ route: "settings", wiping: true });
    applyLink({ kind: "invitation" });
    expect(ui.get().route).toBe("settings");
  });
});
