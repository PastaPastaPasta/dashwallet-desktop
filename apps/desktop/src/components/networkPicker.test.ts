import { describe, expect, it } from "vitest";
import { NetworkText } from "../network/choice";
import { nextStartNetwork, selectedNetwork } from "./networkPicker";

const testnet = { kind: "testnet", dirName: "testnet", unitName: "tDASH" } as const;
const regtest = { kind: "regtest", dirName: "regtest", unitName: "DASH" } as const;

describe("network picker selection", () => {
  it("selects the running network even when the saved choice is another", () => {
    expect(selectedNetwork({ active: testnet })).toBe("testnet");
    expect(nextStartNetwork({ active: testnet, chosen: "mainnet", forcedByFlag: true })).toBe("mainnet");
    expect(NetworkText.pinned("testnet", "mainnet")).toBe("Testnet is in use for this session. Mainnet will be used the next time DashPay Desktop starts normally.");
  });

  it("says nothing different about the next start when the saved choice is the running network", () => {
    expect(nextStartNetwork({ active: testnet, chosen: "testnet", forcedByFlag: true })).toBeNull();
    expect(NetworkText.pinned("testnet", null)).not.toContain("--");
  });

  it("an unpinned window has no next-start note", () => {
    expect(nextStartNetwork({ active: testnet, chosen: "mainnet", forcedByFlag: false })).toBeNull();
  });

  it("selects no segment for a network the picker does not offer, and names the saved one for the next start", () => {
    expect(selectedNetwork({ active: regtest })).toBeNull();
    expect(nextStartNetwork({ active: regtest, chosen: "mainnet", forcedByFlag: true })).toBe("mainnet");
    expect(NetworkText.pinned("regtest", "mainnet")).toBe("Regtest is in use for this session. Mainnet will be used the next time DashPay Desktop starts normally.");
  });
});
