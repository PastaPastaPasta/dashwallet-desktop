// fillSecret (driver.ts), the first line against a secret in a screenshot: it types only into a
// masked field with no reveal toggle beside it. And no flow types a secret any other way.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { WebDriver, fillSecret as FillSecret } from "./driver.ts";

/** A stand-in DOM node: enough of Element for maskedFieldProblem. */
interface Node {
  tagName: string;
  id: string;
  type?: string;
  textContent: string;
  labels?: Node[];
  attrs: Record<string, string>;
  children: Node[];
  parentElement: Node | null;
  form: Node | null;
  hasAttribute(a: string): boolean;
  getAttribute(a: string): string | null;
  closest(sel: string): Node | null;
  querySelectorAll(sel: string): Node[];
}

const matches = (n: Node, sel: string) =>
  sel.split(",").some((one) => {
    const m = /^\s*([a-z]*)(?:\[(\w+)=(\w+)\])?\s*$/.exec(one)!;
    return (!m[1] || n.tagName.toLowerCase() === m[1]) && (!m[2] || n.getAttribute(m[2]) === m[3]);
  });

function node(tag: string, attrs: Record<string, string> = {}, children: Node[] = []): Node {
  const n: Node = {
    tagName: tag.toUpperCase(),
    id: attrs.id ?? "",
    type: tag === "input" ? (attrs.type ?? "text") : undefined,
    textContent: attrs.text ?? "",
    attrs,
    children,
    parentElement: null,
    form: null,
    hasAttribute: (a) => a in attrs,
    getAttribute: (a) => attrs[a] ?? null,
    closest: (sel) => {
      for (let p: Node | null = n; p; p = p.parentElement) if (matches(p, sel)) return p;
      return null;
    },
    querySelectorAll: (sel) => n.children.flatMap((c) => [...(matches(c, sel) ? [c] : []), ...c.querySelectorAll(sel)]),
  };
  for (const c of children) {
    c.parentElement = n;
    if (tag === "label" && c.tagName === "INPUT") c.labels = [n];
  }
  return n;
}

/** A document whose only field is `field`, inside a form with `beside` next to it, the form inside
 * a dialog with `around` next to it. */
function page(field: Node, beside: Node[] = [], around: Node[] = []) {
  const form = node("form", {}, [node("div", {}, [field]), ...beside]);
  node("div", { role: "dialog" }, [form, ...around]);
  if (field.tagName === "INPUT") field.form = form;
  return { querySelector: (css: string) => (css === "#f" ? field : null) };
}

/** A stand-in WebDriver whose script runs against `doc` and records what is typed. */
function driver(doc: unknown) {
  const typed: string[] = [];
  const wd = {
    waitFor: async () => "el-1",
    script: async (js: string, args: unknown[] = []) => new Function("args", "document", `return (function () { ${js} }).apply(null, args)`)(args, doc),
    type: async (_el: string, text: string) => void typed.push(text),
  } as unknown as WebDriver;
  return { wd, typed };
}

describe("fillSecret", () => {
  let fillSecret: typeof FillSecret;
  const saved = { ...process.env };
  beforeAll(async () => {
    Object.assign(process.env, { DW_APP: "/nonexistent/app", DWCLI: "/nonexistent/dwcli" });
    ({ fillSecret } = await import("./driver.ts"));
  });
  afterAll(() => {
    process.env = { ...saved };
  });

  it("refuses a text input and types nothing", async () => {
    const { wd, typed } = driver(page(node("input", { id: "f", type: "text" })));
    await expect(fillSecret(wd, "#f", "s3cret-value")).rejects.toThrow(/refusing to type a secret into #f: not a masked field \(input type=text\)/);
    expect(typed).toEqual([]);
  });

  it("refuses a textarea, and a field that is not there", async () => {
    const { wd, typed } = driver(page(node("textarea", { id: "f" })));
    await expect(fillSecret(wd, "#f", "s3cret-value")).rejects.toThrow(/not a masked field \(textarea\)/);
    await expect(fillSecret(wd, "#g", "s3cret-value")).rejects.toThrow(/no such field/);
    expect(typed).toEqual([]);
  });

  it("refuses a password input with a reveal toggle in its form", async () => {
    for (const toggle of [
      node("button", { "aria-label": "Show passphrase" }),
      node("button", { "aria-controls": "f" }),
      node("span", { role: "switch", title: "Reveal" }),
      node("button", { "data-reveal": "" }),
      node("button", { text: "Show" }),
      node("label", { text: "Show passphrase" }, [node("input", { type: "checkbox" })]),
    ]) {
      const { wd, typed } = driver(page(node("input", { id: "f", type: "password" }), [toggle]));
      await expect(fillSecret(wd, "#f", "s3cret-value")).rejects.toThrow(/a reveal toggle beside it/);
      expect(typed).toEqual([]);
    }
    const { wd, typed } = driver(page(node("input", { id: "f", type: "password" }), [], [node("button", { "aria-label": "Reveal passphrase" })]));
    await expect(fillSecret(wd, "#f", "s3cret-value")).rejects.toThrow(/a reveal toggle beside it/);
    expect(typed).toEqual([]);
  });

  it("types into a password input with only ordinary buttons beside it", async () => {
    const { wd, typed } = driver(page(node("input", { id: "f", type: "password" }), [node("label", { text: "For mixing only" }, [node("input", { type: "checkbox" })]), node("button", { text: "Unlock" }), node("button", {})], [node("button", { text: "Cancel" })]));
    await fillSecret(wd, "#f", "s3cret-value", "<clear>");
    expect(typed).toEqual(["<clear>s3cret-value"]);
  });
});

describe("the testnet flows", () => {
  const FLOWS = ["dashpay-status", "contacts", "add-contact", "bell-accept", "pay-contact", "profile-write", "registration"];
  const SECRET = /passphrase|phrase|mnemonic|words|secret/i;

  it("type a passphrase or phrase only through fillSecret", () => {
    for (const flow of FLOWS) {
      const src = readFileSync(join(import.meta.dirname, `${flow}.ts`), "utf8");
      const plain = [...src.matchAll(/(?:\.fill|\.type|\benter|\bretype)\(([^;]*?)\);/g)].map((m) => m[0]).filter((call) => SECRET.test(call));
      expect(plain, flow).toEqual([]);
      expect(src, flow).toMatch(/\b(?:fillSecret|enterSecret)\(/);
    }
  });
});
