import { describe, expect, it } from "vitest";
import { installContextMenuGuard } from "./contextMenuGuard";

describe("DEC1058_contextMenuGuard", () => {
  const menu = (doc: EventTarget, target: object) => {
    const e = new Event("contextmenu", { cancelable: true });
    Object.defineProperty(e, "target", { value: target });
    doc.dispatchEvent(e);
    return e.defaultPrevented;
  };

  it("a release build has no menu on the page, and keeps a text field's", () => {
    const doc = new EventTarget();
    installContextMenuGuard(true, doc as Document);
    expect(menu(doc, { tagName: "DIV" })).toBe(true);
    expect(menu(doc, { tagName: "BUTTON" })).toBe(true);
    expect(menu(doc, { tagName: "INPUT", type: "checkbox" })).toBe(true);
    expect(menu(doc, { tagName: "INPUT", type: "text" })).toBe(false);
    expect(menu(doc, { tagName: "INPUT", type: "password" })).toBe(false);
    expect(menu(doc, { tagName: "TEXTAREA" })).toBe(false);
    expect(menu(doc, { isContentEditable: true })).toBe(false);
  });

  it("a development build keeps every menu, and uninstalling restores them", () => {
    const dev = new EventTarget();
    installContextMenuGuard(false, dev as Document);
    expect(menu(dev, { tagName: "DIV" })).toBe(false);
    const doc = new EventTarget();
    const off = installContextMenuGuard(true, doc as Document);
    off();
    expect(menu(doc, { tagName: "DIV" })).toBe(false);
  });
});
