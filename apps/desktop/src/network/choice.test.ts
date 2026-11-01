import { describe, expect, it } from "vitest";
import { pinned, restartNeeded } from "./choice";

describe("network choice", () => {
  it("asks for a restart only when the saved choice is not the window's network", () => {
    expect(restartNeeded("mainnet", "mainnet")).toBe(false);
    expect(restartNeeded("testnet", "testnet")).toBe(false);
    expect(restartNeeded("mainnet", "testnet")).toBe(true);
    expect(restartNeeded("testnet", "mainnet")).toBe(true);
  });

  it("asks for nothing while the choice is unknown, or in a window on neither choice", () => {
    expect(restartNeeded(null, "testnet")).toBe(false);
    expect(restartNeeded(undefined, "mainnet")).toBe(false);
    expect(restartNeeded("mainnet", "regtest")).toBe(false);
    expect(restartNeeded("testnet", "devnet")).toBe(false);
    expect(pinned("regtest")).toBe(true);
    expect(pinned("devnet")).toBe(true);
    expect(pinned("mainnet")).toBe(false);
    expect(pinned("testnet")).toBe(false);
  });
});
