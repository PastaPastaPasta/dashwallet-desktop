// DP2-08 D11: a drop outside a drop area or a text field does nothing.

import { describe, expect, it } from "vitest";
import { droppedLink, installDropGuard, setLinkDrop } from "./dropGuard";

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
    // A file never does, nor anything into a field no one types in (a link dropped there loads).
    expect(send("drop", { target: field, types: ["Files"] }).defaultPrevented).toBe(true);
    for (const type of ["file", "checkbox", "radio", "submit", "range"])
      expect(send("dragover", { target: { tagName: "INPUT", type } }).defaultPrevented, type).toBe(true);
    expect(send("drop", { target: { ...field, readOnly: true } }).defaultPrevented).toBe(true);
    expect(send("drop", { target: { tagName: "TEXTAREA", disabled: true } }).defaultPrevented).toBe(true);
  });

  it("QT-019 opens a dropped link outside a text field; a drop area, a text field and a file keep theirs", () => {
    const doc = new EventTarget();
    installDropGuard(doc as Document);
    const opened: string[] = [];
    setLinkDrop((text) => opened.push(text));
    const drop = (data: Record<string, string>, { area = false, target = {}, files = false }: { area?: boolean; target?: object; files?: boolean } = {}) => {
      const e = new Event("drop", { cancelable: true });
      Object.defineProperty(e, "target", { value: target });
      const types = [...Object.keys(data), ...(files ? ["Files"] : [])];
      Object.defineProperty(e, "dataTransfer", { value: { types, dropEffect: "copy", getData: (t: string) => data[t] ?? "" } });
      if (area) e.preventDefault();
      doc.dispatchEvent(e);
      return e;
    };
    const uri = "dash:yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd?amount=0.5";
    expect(drop({ "text/uri-list": `# from a page\n${uri}\n` }).defaultPrevented).toBe(true);
    drop({ "text/plain": `  ${uri}  ` });
    expect(opened).toEqual([uri, uri]);
    // Not a link of the app's, a link with spaces, a drop area's, a text field's, a file: nothing opens.
    drop({ "text/plain": "https://example.org" });
    drop({ "text/plain": "dash:abc def" });
    drop({ "text/uri-list": uri }, { area: true });
    drop({ "text/uri-list": uri }, { target: { tagName: "INPUT", type: "text" } });
    drop({ "text/uri-list": uri }, { files: true });
    expect(opened).toHaveLength(2);
    // The cursor says a link drop is taken, and no drop is while no one opens links.
    const over = (types: string[]) => {
      const e = new Event("dragover", { cancelable: true });
      Object.defineProperty(e, "target", { value: {} });
      Object.defineProperty(e, "dataTransfer", { value: { types, dropEffect: "move" } });
      doc.dispatchEvent(e);
      return (e as Event & { dataTransfer: { dropEffect: string } }).dataTransfer.dropEffect;
    };
    expect(over(["text/uri-list"])).toBe("copy");
    expect(over(["Files", "text/uri-list"])).toBe("none");
    setLinkDrop(null);
    expect(over(["text/uri-list"])).toBe("none");
    drop({ "text/uri-list": uri });
    expect(opened).toHaveLength(2);
  });

  it("reads a link from a URI list's first entry, else the plain text", () => {
    const data = (d: Record<string, string>) => ({ getData: (t: string) => d[t] ?? "" });
    expect(droppedLink(data({ "text/uri-list": "#c\r\ndashpay:contact\r\nother:x" }))).toBe("dashpay:contact");
    expect(droppedLink(data({ "text/plain": "PAY:yAbc" }))).toBe("PAY:yAbc");
    expect(droppedLink(data({ "text/plain": "dash:" }))).toBeNull();
    expect(droppedLink(null)).toBeNull();
  });
});
