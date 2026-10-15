// DP2-08 D11: a drop outside a drop area or a text field does nothing.

import { describe, expect, it } from "vitest";
import { installDropGuard } from "./dropGuard";

describe("installDropGuard", () => {
  it("D11 cancels drops the page did not take; a drop area's and a text field's text are left alone", () => {
    const doc = new EventTarget();
    installDropGuard(doc as Document);
    const send = (type: string, { area = false, target = {}, types = ["text/uri-list"] }: { area?: boolean; target?: object; types?: string[] } = {}) => {
      const e = new Event(type, { cancelable: true });
      Object.defineProperty(e, "target", { value: target });
      Object.defineProperty(e, "dataTransfer", { value: { types, dropEffect: "copy" } });
      // A drop area's handler runs first and cancels the default itself.
      if (area) e.preventDefault();
      doc.dispatchEvent(e);
      return e as Event & { dataTransfer: { dropEffect: string } };
    };
    expect(send("drop").defaultPrevented).toBe(true);
    const over = send("dragover");
    expect(over.defaultPrevented).toBe(true);
    expect(over.dataTransfer.dropEffect).toBe("none");
    expect(send("dragover", { area: true }).dataTransfer.dropEffect).toBe("copy");
    // A dragged text or link goes into a text field as typed.
    const field = { tagName: "INPUT", type: "text" };
    expect(send("dragover", { target: field }).defaultPrevented).toBe(false);
    expect(send("drop", { target: field }).defaultPrevented).toBe(false);
    expect(send("drop", { target: { tagName: "TEXTAREA" }, types: ["text/plain"] }).defaultPrevented).toBe(false);
    expect(send("drop", { target: { isContentEditable: true } }).defaultPrevented).toBe(false);
    // A file never does, nor anything into a file input.
    expect(send("drop", { target: field, types: ["Files"] }).defaultPrevented).toBe(true);
    expect(send("dragover", { target: { tagName: "INPUT", type: "file" } }).defaultPrevented).toBe(true);
  });
});
