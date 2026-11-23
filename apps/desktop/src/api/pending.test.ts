import { describe, expect, it } from "vitest";
import { commandAbsent } from "./pending";

describe("commands dw-app does not have yet", () => {
  it("recognises Tauri's unknown command and the fixture's", () => {
    expect(commandAbsent("Command get_network_choice not found")).toBe(true);
    expect(commandAbsent("Command get_network_choice not allowed by ACL")).toBe(true);
    expect(commandAbsent("get_network_choice not allowed. Command not found")).toBe(true);
    expect(commandAbsent({ code: "platform.not_implemented", message: "x", data: null })).toBe(true);
  });

  it("does not take another failure for an absent command", () => {
    expect(commandAbsent({ code: "wallet_not_found", message: "x", data: null })).toBe(false);
    expect(commandAbsent("The file could not be read")).toBe(false);
    expect(commandAbsent(new Error("Command x not found"))).toBe(false);
    expect(commandAbsent(null)).toBe(false);
  });
});

describe("integration of the shielded codes (m4 v47)", () => {
  it("SHIELDED_CODES are all generated ShieldedCodes, and isAppError knows every one", async () => {
    const { isAppError } = await import("./backend");
    const { SHIELDED_CODES } = await import("./pending");
    for (const code of Object.values(SHIELDED_CODES)) expect(isAppError({ code, message: "x", data: null }), code).toBe(true);
    expect(Object.values(SHIELDED_CODES)).toContain("shielded.unrecorded_spend");
    expect(isAppError({ code: "shielded.made_up", message: "x", data: null })).toBe(false);
  });
});
