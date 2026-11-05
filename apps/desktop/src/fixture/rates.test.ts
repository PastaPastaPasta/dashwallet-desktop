// The fixture's prices (rates.ts, MP-01) through the generated commands: the engine's states and
// codes, the `rates` event, and no attempt while prices are off.

import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import { installFixture, type FixtureHandle } from "./index";
import { ctxPrices } from "./rates";
import type { ScenarioName } from "./scenarios";

let fixture: FixtureHandle;
const install = (scenario: ScenarioName) => (fixture = installFixture({ scenario }));
beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture prices", () => {
  it("are off on testnet and before the disclosure, with no attempt", async () => {
    install("funded");
    expect((await cmd.ratesStatus()).state).toEqual({ kind: "off", reason: "notDisclosed" });
    await cmd.setRatesSettings({ enabled: true, currency: "USD", disclosed: true });
    const v = await cmd.ratesStatus();
    expect(v.state).toEqual({ kind: "off", reason: "notMainnet" });
    expect(v.rate).toBeNull();
    expect(await cmd.ratesCurrencies()).toEqual([]);
    await expect(cmd.refreshRates()).rejects.toMatchObject({ code: "rates.off" });
    expect(fixture.world.rates.requests).toBe(0);
  });

  it("show each state of the scenarios, a number only when fresh or stale", async () => {
    const cases: [ScenarioName, string, boolean][] = [
      ["fiat", "fresh", true],
      ["fiat-stale", "stale", true],
      ["fiat-volatile", "fresh", true],
      ["fiat-unavailable", "unavailable", false],
      ["fiat-off", "off", false],
      ["fiat-jpy", "fresh", true],
      ["fiat-disclosure", "off", false],
    ];
    for (const [scenario, kind, number] of cases) {
      install(scenario);
      const v = await cmd.ratesStatus();
      expect(v.state.kind, scenario).toBe(kind);
      expect(v.rate !== null, scenario).toBe(number);
      expect(v.volatile, scenario).toBe(scenario === "fiat-volatile");
      clearMocks();
    }
  });

  it("refreshes once per 30 s and announces the change", async () => {
    install("fiat-unavailable");
    fixture.world.rates.lastAttemptAt = null;
    fixture.world.rates.serves = ctxPrices(0);
    const seen: AppEvent[] = [];
    await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => void seen.push(e.payload));
    const v = await cmd.refreshRates();
    expect(v.state.kind).toBe("fresh");
    expect(v.rate?.price).toBe("49.0967");
    await expect(cmd.refreshRates()).rejects.toMatchObject({ code: "rates.throttled" });
    expect(fixture.world.rates.requests).toBe(1);
    await vi.waitFor(() => expect(seen.filter((e) => e.kind === "rates")).toHaveLength(1));
  });

  it("refuses a code that is not three capital letters", async () => {
    install("fiat");
    await expect(cmd.setRatesSettings({ enabled: true, currency: "usd", disclosed: true })).rejects.toMatchObject({ code: "rates.invalid_currency" });
  });

  it("turning off stops attempts", async () => {
    install("fiat");
    fixture.world.rates.lastAttemptAt = null;
    await cmd.setRatesSettings({ enabled: false, currency: "USD", disclosed: true });
    await expect(cmd.refreshRates()).rejects.toMatchObject({ code: "rates.off" });
    expect(await cmd.ratesCurrencies()).toEqual([]);
    expect(fixture.world.rates.requests).toBe(0);
  });

  it("stamps the newest rows on Mainnet, each at its own time; none before the disclosure or on testnet", async () => {
    install("fiat");
    const walletId = fixture.world.wallets[0].summary.walletId;
    const rows = fixture.world.wallets[0].rows;
    const stamps = rows.slice(0, 5).map((r) => r.fiatThen?.currency ?? null);
    expect(stamps).toEqual(["USD", null, "EUR", "USD", "USD"]);
    for (const r of rows.filter((r) => r.fiatThen)) expect(r.fiatThen!.rateAt).toBe(r.timestamp);
    const detail = await cmd.txDetail(walletId, rows[0].txid);
    expect(detail.rows[0].fiatThen).toEqual(rows[0].fiatThen);
    for (const s of ["funded", "fiat-disclosure"] as const) {
      install(s);
      expect(fixture.world.wallets[0].rows.some((r) => r.fiatThen)).toBe(false);
    }
  });
});
