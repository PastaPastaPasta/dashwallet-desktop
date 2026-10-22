// The wipe sheet holds the shell while it is open (U-04 D38).

import { afterEach, describe, expect, it, vi } from "vitest";
import { applyLink, linkProblem, runCommand, traceName } from "./shellBridge";
import { ui } from "./ui";

const initial = ui.get();

afterEach(() => {
  ui.set(initial);
  vi.unstubAllGlobals();
});

describe("while the wipe sheet is open", () => {
  it("IOS109_aMenuCommandLeavesThePageInPlace", () => {
    vi.stubGlobal("requestAnimationFrame", () => 0);
    vi.stubGlobal("cancelAnimationFrame", () => undefined);
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
    applyLink({ kind: "invitation", linkId: "" });
    expect(ui.get().route).toBe("settings");
  });
});

// The page a link or a menu opens takes the focus one frame later, unless the user moved it
// with a key or a pointer meanwhile (BL-165).
describe("focus after a link or a menu opens a page", () => {
  /** A document whose page has an `h1`, the frames asked for (run by hand; `cancels: false`
   * keeps cancelled ones, as a late frame's timer does), and its key and pointer listeners (fired
   * by hand). */
  function shell({ cancels = true } = {}) {
    const frames = new Map<number, FrameRequestCallback>();
    let id = 0;
    vi.stubGlobal("requestAnimationFrame", (f: FrameRequestCallback) => (frames.set(++id, f), id));
    vi.stubGlobal("cancelAnimationFrame", (n: number) => void (cancels && frames.delete(n)));
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
    const nextFrame = () => {
      const due = [...frames.values()];
      frames.clear();
      due.forEach((f) => f(0));
    };
    const fire = (type: string) => listeners.get(type)?.forEach((f) => f());
    const listening = (type?: string) => [...listeners].reduce((n, [t, l]) => n + (type === undefined || t === type ? l.size : 0), 0);
    return { doc, heading, nextFrame, fire, frames, listening };
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
    applyLink({ kind: "invitation", linkId: "" });
    doc.activeElement = { name: "sidebar-pay" };
    nextFrame();
    expect(doc.activeElement).toBe(heading);
    expect(listening()).toBe(0);
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
      expect(listening()).toBe(0);
    });
  }

  it("goes to the heading after a key that moved no focus (a modifier over a sheet's opener)", () => {
    const { doc, heading, nextFrame, fire } = shell();
    doc.activeElement = { name: "open-uri-input" };
    applyLink({ kind: "invitation", linkId: "" });
    doc.activeElement = { name: "sidebar-pay" };
    fire("keydown");
    nextFrame();
    expect(doc.activeElement).toBe(heading);
  });

  it("goes to the heading after a key that left focus on the body", () => {
    const { doc, heading, nextFrame, fire } = shell();
    runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
    fire("keydown");
    nextFrame();
    expect(doc.activeElement).toBe(heading);
  });

  for (const cancels of [true, false]) {
    it(`keeps one wait when a newer page opens before the frame${cancels ? "" : " (a frame that cannot be cancelled)"}`, () => {
      const { doc, heading, nextFrame, fire, frames, listening } = shell({ cancels });
      runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
      fire("keydown");
      doc.activeElement = { name: "row" };
      applyLink({ kind: "invitation", linkId: "" });
      runCommand({ kind: "navigate", route: "contacts" }, () => undefined);
      expect(listening("keydown")).toBe(1);
      expect(listening("pointerdown")).toBe(1);
      if (cancels) expect(frames.size).toBe(1);
      // A key before the newer page opened is not one since: that page takes the focus, as it
      // would have had every frame come on time.
      nextFrame();
      expect(doc.activeElement).toBe(heading);
      expect(listening()).toBe(0);
    });
  }

  it("a frame replaced but not cancelled leaves the newer wait in place", () => {
    const { doc, nextFrame, fire, listening } = shell({ cancels: false });
    runCommand({ kind: "navigate", route: "notifications" }, () => undefined);
    nextFrame(); // the first page lands
    runCommand({ kind: "navigate", route: "contacts" }, () => undefined);
    runCommand({ kind: "navigate", route: "activity" }, () => undefined);
    fire("pointerdown");
    const row = { name: "row" };
    doc.activeElement = row;
    nextFrame(); // the replaced frame runs first and must not drop the newer one's input
    expect(doc.activeElement).toBe(row);
    expect(listening()).toBe(0);
  });
});

// DP5-01: links kept in the stash.
describe("stashed links", () => {
  const saved = { kind: "saved", link: { id: "1".padStart(32, "0"), kind: "payment", stashedAt: 1, expiresAt: 2 } } as const;

  it("DP5-01 a saved link opens nothing and is named without its contents", () => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    ui.set({ route: "home", onboarding: true });
    applyLink(saved);
    expect(ui.get().route).toBe("home");
    expect(linkProblem(saved)).toBeNull();
    expect(traceName(saved)).toBe("saved");
    expect(traceName({ kind: "invitation", linkId: "f".repeat(32) })).toBe("invitation");
  });

  it("DP5-01 an invitation during onboarding stays there; later it opens the claim screen (DP5-03 D3)", () => {
    vi.stubGlobal("window", { setTimeout: () => 0 });
    vi.stubGlobal("requestAnimationFrame", () => 0);
    vi.stubGlobal("cancelAnimationFrame", () => undefined);
    vi.stubGlobal("document", { activeElement: null, addEventListener: () => undefined, removeEventListener: () => undefined });
    ui.set({ route: "home", onboarding: true });
    applyLink({ kind: "invitation", linkId: "1".padStart(32, "0") });
    expect(ui.get().route).toBe("home");
    ui.set({ onboarding: false });
    applyLink({ kind: "invitation", linkId: "1".padStart(32, "0") });
    expect(ui.get()).toMatchObject({ route: "joinDashPay", invitation: { linkId: "1".padStart(32, "0"), entry: "join" } });
  });

  it("DP5-01 an invitation that could not be kept says so", () => {
    const route = { kind: "rejected", reason: { kind: "notSaved" } } as const;
    expect(linkProblem(route)).toMatch(/could not be saved/);
    expect(traceName(route)).toBe("rejected notSaved");
  });
});
