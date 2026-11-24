// A held sheet's credential (issue 635): it signs nothing, so it reads neither the lock nor the
// payment policy. React DOM renders the hook into a stub container (vitest runs in node).

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../../api/backend";
import { BackendContext } from "../../api/queries";
import { useCredential, type Credential } from "./credential";

let calls: string[];
const backend = new Proxy({} as Backend, {
  get: (_, name: string) => () => {
    calls.push(name);
    if (name === "lockStatus") return Promise.resolve({ encrypted: true, locked: false, retryAfterSecs: null });
    if (name === "paymentAuthentication") return Promise.resolve(true);
    return Promise.resolve(null);
  },
});

let root: Root;
let seen: Credential | null;
function Probe({ active }: { active: boolean }) {
  seen = useCredential("w", "ask", 1000, active);
  return null;
}
const show = (active: boolean) =>
  act(async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    root.render(createElement(QueryClientProvider, { client }, createElement(BackendContext.Provider, { value: backend }, createElement(Probe, { active }))));
  });

describe("useCredential", () => {
  beforeEach(() => {
    const noop = () => {};
    calls = [];
    seen = null;
    vi.stubGlobal("window", { HTMLIFrameElement: class {}, setInterval, clearInterval });
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    const doc = { addEventListener: noop, removeEventListener: noop };
    root = createRoot({ nodeType: 1, tagName: "DIV", textContent: "", ownerDocument: doc, addEventListener: noop, removeEventListener: noop } as unknown as Element);
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    vi.unstubAllGlobals();
  });

  it("held (inactive): reads nothing and is never ready", async () => {
    await show(false);
    expect(calls).toEqual([]);
    expect(seen?.ready).toBe(false);
  });

  it("active: reads the lock's wait and the payment policy", async () => {
    await show(true);
    expect(calls).toContain("lockStatus");
    expect(calls).toContain("paymentAuthentication");
    expect(seen?.ready).toBe(true);
  });
});
