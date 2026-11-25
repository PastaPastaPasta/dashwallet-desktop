// The Receive address read is read again once when the account reads bound after it failed, and
// not again while it keeps failing: TanStack Query clears a data-less query's error while it
// refetches, so a re-read keyed on the error alone would run again on every failure.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useRereadWhenBound } from "./queries";

let calls = 0;
function Probe({ bound }: { bound: boolean }) {
  const read = useQuery({
    queryKey: ["address"],
    queryFn: async () => {
      calls += 1;
      // As IPC answers: a moment later, so the pending state between reads renders.
      await new Promise((r) => setTimeout(r, 10));
      throw new Error("not synced");
    },
    retry: false,
  });
  useRereadWhenBound(bound, true, read.isError, read.refetch);
  return null;
}

let root: Root;
let client: QueryClient;
const render = (bound: boolean) => act(async () => root.render(createElement(QueryClientProvider, { client }, createElement(Probe, { bound }))));
/** Until no read has run for ten quiet turns (a loop never gets there), or 3 s. */
const settle = async () => {
  for (let quiet = 0, i = 0; quiet < 10 && i < 300; i++) {
    await act(async () => new Promise((r) => setTimeout(r, 10)));
    quiet = client.isFetching() === 0 ? quiet + 1 : 0;
  }
};

describe("the re-read when the account reads bound", () => {
  beforeEach(() => {
    const noop = () => {};
    vi.stubGlobal("window", { HTMLIFrameElement: class {} });
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    const doc = { addEventListener: noop, removeEventListener: noop };
    root = createRoot({ nodeType: 1, tagName: "DIV", textContent: "", ownerDocument: doc, addEventListener: noop, removeEventListener: noop } as unknown as Element);
    client = new QueryClient();
    calls = 0;
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    client.clear();
    vi.unstubAllGlobals();
  });

  it("reads a read that keeps failing once more per bound state, never in a loop", async () => {
    await render(false);
    await settle();
    expect(calls).toBe(1);
    await render(true);
    await settle();
    expect(calls).toBe(2);
    await render(false);
    await settle();
    await render(true);
    await settle();
    expect(calls).toBe(3);
  });

  it("reads a read that fails while already bound once more, and then stops", async () => {
    await render(true);
    await settle();
    expect(calls).toBe(2);
  });
});
