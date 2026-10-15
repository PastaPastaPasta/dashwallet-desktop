// DP2-08 D11: a drop outside a drop area does nothing.

import { describe, expect, it } from "vitest";
import { installDropGuard } from "./dropGuard";

describe("installDropGuard", () => {
  it("D11 cancels drops the page did not take, and leaves a drop area's alone", () => {
    const doc = new EventTarget();
    installDropGuard(doc as Document);
    const send = (type: string, area: boolean) => {
      const e = new Event(type, { cancelable: true });
      // A drop area's handler runs first and cancels the default itself.
      if (area) e.preventDefault();
      doc.dispatchEvent(e);
      return e;
    };
    expect(send("drop", false).defaultPrevented).toBe(true);
    expect(send("dragover", false).defaultPrevented).toBe(true);
    expect(send("dragover", true).defaultPrevented).toBe(true);
  });
});
