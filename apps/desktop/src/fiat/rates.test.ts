import { describe, expect, it } from "vitest";
import type { RatesView } from "../api/generated/RatesView";
import { ratesNote, ratesStatusText, shownRate } from "./rates";

const AT = 1_793_835_763;
const view = (state: RatesView["state"], over: Partial<RatesView> = {}): RatesView => ({
  settings: { enabled: true, currency: "USD", disclosed: true },
  state,
  rate: state.kind === "fresh" || state.kind === "stale" ? { currency: "USD", price: "49.0967", rateAt: AT, source: "ctx" } : null,
  volatile: false,
  lastAttemptFailed: false,
  lastAttemptAt: AT,
  lastSuccessAt: AT,
  ...over,
});

const OFF: RatesView["state"][] = [
  { kind: "off", reason: "disabled" },
  { kind: "off", reason: "notDisclosed" },
  { kind: "off", reason: "notMainnet" },
];
const UNAVAILABLE: RatesView["state"][] = (["neverFetched", "failed", "expired", "noCurrency", "currencyMissing"] as const).map((reason) => ({ kind: "unavailable", reason }));

describe("which price may be shown", () => {
  it("only a fresh or stale one", () => {
    expect(shownRate(view({ kind: "fresh" }))).toMatchObject({ price: "49.0967", stale: false });
    expect(shownRate(view({ kind: "stale" }))).toMatchObject({ price: "49.0967", stale: true });
    for (const s of [...OFF, ...UNAVAILABLE]) expect(shownRate(view(s))).toBeNull();
    // A rate the state does not allow is never used.
    expect(shownRate(view({ kind: "unavailable", reason: "expired" }, { rate: { currency: "USD", price: "1", rateAt: AT, source: "ctx" } }))).toBeNull();
    expect(shownRate(undefined)).toBeNull();
  });
});

describe("what the hero says about prices", () => {
  it("nothing when off or fresh, a caution when stale or volatile, a quiet line when unavailable", () => {
    for (const s of OFF) expect(ratesNote(view(s))).toBeNull();
    expect(ratesNote(view({ kind: "fresh" }))).toBeNull();
    expect(ratesNote(view({ kind: "fresh" }, { volatile: true }))).toMatchObject({ tone: "caution", text: expect.stringContaining("50%") });
    expect(ratesNote(view({ kind: "stale" }))).toMatchObject({ tone: "caution", text: "Prices are over 30 minutes old." });
    for (const s of UNAVAILABLE) expect(ratesNote(view(s))).toMatchObject({ tone: "quiet", text: "Prices unavailable", title: expect.any(String) });
  });
});

describe("the Settings status line", () => {
  it("names the source and the time, and a failed attempt", () => {
    expect(ratesStatusText(view({ kind: "fresh" }), "en-US")).toMatch(/^Updated .+ from rates\.ctx\.com\.$/);
    expect(ratesStatusText(view({ kind: "stale" }, { lastAttemptFailed: true }), "en-US")).toMatch(/over 30 minutes old\. Last update failed at /);
    const fallback = view({ kind: "fresh" });
    fallback.rate!.source = "bitPayDashCentral";
    expect(ratesStatusText(fallback, "en-US")).toContain("BitPay and DashCentral");
    expect(ratesStatusText(view({ kind: "off", reason: "notMainnet" }), "en-US")).toContain("Mainnet");
    expect(ratesStatusText(view({ kind: "off", reason: "disabled" }), "en-US")).toBeNull();
    expect(ratesStatusText(view({ kind: "unavailable", reason: "failed" }), "en-US")).toContain("could not be fetched");
  });
});
