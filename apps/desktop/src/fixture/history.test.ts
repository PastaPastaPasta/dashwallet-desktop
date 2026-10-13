// The fixture's `history_page` against the engine's: every case of vectors/history.json,
// which dw-app generates from `dw_engine::history::page` (rust/crates/dw-app/src/
// history_vectors_tests.rs; CI fails when it is stale). Regenerate with
// `DW_UPDATE_VECTORS=1 cargo test -p dw-app history_vectors`.

import { clearMocks } from "@tauri-apps/api/mocks";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError } from "../api/generated/AppError";
import type { HistoryRequest } from "../api/generated/HistoryRequest";
import type { HistoryRows } from "../api/generated/HistoryRows";
import type { TxRow } from "../api/generated/TxRow";
import { IpcBackend } from "../api/tauri";
import { buildRows, historyPage } from "./wallet";
import { installFixture } from "./index";
import { FIXTURE_WALLET_ID } from "./scenarios";

interface Case {
  name: string;
  request: HistoryRequest;
  response?: HistoryRows;
  error?: Pick<AppError, "code" | "data">;
}

const vectors = JSON.parse(readFileSync(new URL("./vectors/history.json", import.meta.url), "utf8")) as {
  rows: TxRow[];
  cases: Case[];
};

function outcome(request: HistoryRequest) {
  try {
    return { response: historyPage(vectors.rows, request) };
  } catch (e) {
    const { code, data } = e as AppError;
    return { error: { code, data } };
  }
}

describe("fixture history_page matches the engine", () => {
  it("has the vectors", () => {
    expect(vectors.rows.length).toBeGreaterThanOrEqual(30);
    expect(vectors.cases.length).toBeGreaterThan(100);
    expect(vectors.cases.some((c) => c.error?.code === "stale_cursor")).toBe(true);
    expect(vectors.cases.some((c) => c.error?.code === "invalid_query")).toBe(true);
  });

  it.each(vectors.cases.map((c) => [c.name, c] as const))("%s", (_, c) => {
    const expected = c.error ? { error: c.error } : { response: c.response };
    expect(outcome(c.request)).toEqual(expected);
  });

  it("leaves the rows it was given in their order", () => {
    const rows = structuredClone(vectors.rows);
    historyPage(rows, { categories: [], text: null, sort: "amountAscending", cursor: null, limit: 5 });
    expect(rows).toEqual(vectors.rows);
  });

  it("pages the large scenario quickly", () => {
    const rows = buildRows("large");
    const started = performance.now();
    const page = historyPage(rows, { categories: [], text: null, sort: "amountDescending", cursor: null, limit: 200 });
    historyPage(rows, { categories: [], text: null, sort: "amountDescending", cursor: page.nextCursor, limit: 200 });
    // Generous: the box running the tests may be loaded.
    expect(performance.now() - started).toBeLessThan(1_000);
  });
});

describe("fixture history_page over IPC", () => {
  beforeEach(() => vi.stubGlobal("window", globalThis));
  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  function backend() {
    const fixture = installFixture({ scenario: "funded" });
    fixture.world.wallets[0].rows = vectors.rows;
    return new IpcBackend("fixture", () => fixture.now, fixture.locale);
  }

  it("answers a vector case through the handler", async () => {
    const c = vectors.cases.find((x) => x.response?.nextCursor && x.request.cursor)!;
    expect(await backend().historyPage(FIXTURE_WALLET_ID, c.request)).toEqual(c.response);
  });

  it("rejects a stale cursor with the engine's AppError", async () => {
    const c = vectors.cases.find((x) => x.error?.code === "stale_cursor")!;
    await expect(backend().historyPage(FIXTURE_WALLET_ID, c.request)).rejects.toMatchObject(c.error!);
  });
});
