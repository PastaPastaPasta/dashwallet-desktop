import { describe, expect, it } from "vitest";
import { commandAbsent } from "./pending";

describe("commands dw-app does not have yet", () => {
  it("recognises Tauri's unknown command and the fixture's", () => {
    expect(commandAbsent("Command get_network_choice not found")).toBe(true);
    expect(commandAbsent({ code: "platform.not_implemented", message: "x", data: null })).toBe(true);
  });

  it("does not take another failure for an absent command", () => {
    expect(commandAbsent({ code: "wallet_not_found", message: "x", data: null })).toBe(false);
    expect(commandAbsent("The file could not be read")).toBe(false);
    expect(commandAbsent(new Error("Command x not found"))).toBe(false);
    expect(commandAbsent(null)).toBe(false);
  });
});
