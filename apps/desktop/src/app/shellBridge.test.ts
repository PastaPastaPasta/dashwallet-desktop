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
    vi.stubGlobal("document", { activeElement: null, addEventListener: () => undefined, removeEventListener: () => undefined });
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

// The page a link or a menu opens takes the focus one frame later, unless the user moved it
// meanwhile (BL-165).
describe("focus after a link or a menu opens a page", () => {
  /** A document whose page has an `h1`, the frames asked for (run by hand), and its key and
   * pointer listeners (fired by hand). */
  function shell() {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (f: FrameRequestCallback) => frames.push(f));
    const listeners = new Map<string, Set<() => void>>();
    const body = { name: "body" };
    const doc = {
      body,
      activeElement: body as unknown,
      querySelector: () => ({ querySelector: () => heading }),
      addEventListener: (type: string, f: () => void) => void (listeners.get(type) ?? listeners.set(type, new Set()).get(type)!).add(f),
      removeEventListener: (type: string, f: () => void) => void listeners.get(type)?.delete(f),
    };
    const heading = { name: "h1", hasAttribute: () => true, focus: () => void (doc.activeElement = heading) };
    vi.stubGlobal("document", doc);
    const nextFrame = () => frames.splice(0).forEach((f) => f(0));
    const fire = (type: string) => listeners.get(type)?.forEach((f) => f());
    return { doc, heading, nextFrame, fire, listening: () => [...listeners.values()].some((l) => l.size > 0) };
  }

  it("goes to the page's heading when focus is on the body", () => {
    const { doc, heading, nextFrame } = shell();
    runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
    nextFrame();
    expect(doc.activeElement).toBe(heading);
  });

  it("goes to the heading when a closing sheet gave focus back to its opener", () => {
    const { doc, heading, nextFrame, listening } = shell();
    doc.activeElement = { name: "open-uri-input" };
    applyLink({ kind: "invitation" });
    doc.activeElement = { name: "sidebar-pay" };
    nextFrame();
    expect(doc.activeElement).toBe(heading);
    expect(listening()).toBe(false);
  });

  for (const input of ["keydown", "pointerdown"]) {
    it(`leaves focus the user moved with a ${input} before the frame came`, () => {
      const { doc, nextFrame, fire, listening } = shell();
      runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
      fire(input);
      const row = { name: "row" };
      doc.activeElement = row;
      nextFrame();
      expect(doc.activeElement).toBe(row);
      expect(listening()).toBe(false);
    });
  }

  it("goes to the heading after a key that left focus on the body", () => {
    const { doc, heading, nextFrame, fire } = shell();
    runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
    fire("keydown");
    nextFrame();
    expect(doc.activeElement).toBe(heading);
  });
});
