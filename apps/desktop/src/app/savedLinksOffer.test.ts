// DP5-01 D11: when the "Links you opened" offer opens, driven through the hook. React DOM renders
// it into a stub container (vitest runs in node; the hook renders nothing).

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import { noteLiveLink } from "./savedLinks";
import { useSavedLinksOffer } from "./savedLinksOffer";
import { ui } from "./ui";

const backend = {
  stashedLinks: async () => [{ id: "a".repeat(32), kind: "invitation", stashedAt: 1, expiresAt: 2 }],
} as unknown as Backend;

function Offer({ locked }: { locked: boolean }) {
  useSavedLinksOffer(backend, { hasWallet: true, locked, covered: false });
  return null;
}

let root: Root;
/** Renders the window locked or not; the stash read answers before it returns. */
const show = (locked: boolean) => act(async () => root.render(createElement(Offer, { locked })));
const offered = () => ui.get().dialog === "savedLinks";
/** Later: the dialog closes. */
const later = () => ui.set({ dialog: null });

describe("the saved-links offer", () => {
  beforeEach(() => {
    const noop = () => {};
    vi.stubGlobal("window", { HTMLIFrameElement: class {} });
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    const doc = { addEventListener: noop, removeEventListener: noop };
    root = createRoot({ nodeType: 1, tagName: "DIV", textContent: "", ownerDocument: doc, addEventListener: noop, removeEventListener: noop } as unknown as Element);
    later();
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    vi.unstubAllGlobals();
  });

  it("DP5-01 a link applied while locked at launch wins over the offer at unlock; the next unlock offers it", async () => {
    await show(true);
    // The live invitation opens its claim screen behind the lock screen (applyLink).
    noteLiveLink();
    await show(false);
    expect(offered()).toBe(false);
    // Kept in the stash: the next unlock offers it (D11).
    await show(true);
    await show(false);
    expect(offered()).toBe(true);
  });

  it("DP5-01 offers again after a lock and unlock with no new link", async () => {
    await show(true);
    await show(false);
    expect(offered()).toBe(true);
    later();
    await show(true);
    await show(false);
    expect(offered()).toBe(true);
  });
});
