import { describe, expect, it } from "vitest";
import { explorerUrl } from "./explorer";

const TXID = "ab".repeat(32);

describe("explorer URL", () => {
  it("uses Insight on mainnet and testnet, and no link on regtest or devnet", () => {
    expect(explorerUrl("mainnet", TXID)).toBe(`https://insight.dash.org/insight/tx/${TXID}`);
    expect(explorerUrl("testnet", TXID)).toBe(`https://insight.testnet.networks.dash.org:3002/insight/tx/${TXID}`);
    expect(explorerUrl("regtest", TXID)).toBeNull();
    expect(explorerUrl("devnet", TXID)).toBeNull();
  });

  it("refuses a txid that is not 64 lowercase hex characters, as the app does", () => {
    expect(explorerUrl("mainnet", TXID.toUpperCase())).toBeNull();
    expect(explorerUrl("mainnet", TXID.slice(0, 63))).toBeNull();
    expect(explorerUrl("mainnet", `${TXID}ff`)).toBeNull();
    expect(explorerUrl("mainnet", `${TXID.slice(0, 63)}g`)).toBeNull();
    expect(explorerUrl("testnet", "")).toBeNull();
  });
});
