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
    vi.stubGlobal("document", { activeElement: null });
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

// The page a link or a menu opens takes the focus one frame later, unless it moved meanwhile (BL-165).
describe("focus after a link or a menu opens a page", () => {
  /** A document whose page has an `h1`, and the frames asked for, run by hand. */
  function shell() {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (f: FrameRequestCallback) => frames.push(f));
    const doc: { activeElement: unknown; body: unknown; querySelector: () => unknown } = { activeElement: null, body: null, querySelector: () => null };
    const body = { name: "body" };
    const heading = { name: "h1", hasAttribute: () => true, focus: () => void (doc.activeElement = heading) };
    doc.body = body;
    doc.activeElement = body;
    doc.querySelector = () => ({ querySelector: () => heading });
    vi.stubGlobal("document", doc);
    const nextFrame = () => frames.splice(0).forEach((f) => f(0));
    return { doc, heading, nextFrame };
  }

  it("goes to the page's heading when focus is on the body", () => {
    const { doc, heading, nextFrame } = shell();
    runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
    nextFrame();
    expect(doc.activeElement).toBe(heading);
  });

  it("goes to the heading when focus is still where it was", () => {
    const { doc, heading, nextFrame } = shell();
    const toolbar = { name: "toolbar" };
    doc.activeElement = toolbar;
    applyLink({ kind: "invitation" });
    nextFrame();
    expect(doc.activeElement).toBe(heading);
  });

  it("leaves focus the user moved before the frame came", () => {
    const { doc, nextFrame } = shell();
    runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
    const row = { name: "row" };
    doc.activeElement = row;
    nextFrame();
    expect(doc.activeElement).toBe(row);
  });
});
