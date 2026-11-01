import { describe, expect, it } from "vitest";
import { csvFileName, exportHistory, utcOffsetSecs, type CsvFilter } from "./exportCsv";

describe("Export CSV", () => {
  it("names the file with the local date", () => {
    expect(csvFileName(new Date(2026, 10, 1, 23, 59))).toBe("Dash Wallet history 2026-11-01.csv");
    expect(csvFileName(new Date(2026, 0, 9))).toBe("Dash Wallet history 2026-01-09.csv");
  });

  it("gives the offset east of UTC in seconds", () => {
    const now = new Date(2026, 10, 1, 12);
    expect(utcOffsetSecs(now)).toBe(-now.getTimezoneOffset() * 60);
  });

  it("asks the engine for the selected history and saves what it answers", async () => {
    const calls: unknown[][] = [];
    const saved: unknown[][] = [];
    const filter: CsvFilter = { sort: "oldestFirst", categories: ["received"], text: "alice", minAmount: 5 };
    const path = await exportHistory(
      {
        exportHistoryCsv: async (...args) => (calls.push(args), '"a"\n'),
        saveFileAs: async (...args) => (saved.push(args), "/home/user/h.csv"),
      },
      "w1",
      filter,
      "milliDash",
      new Date(2026, 10, 1),
    );
    expect(path).toBe("/home/user/h.csv");
    expect(calls).toEqual([["w1", { categories: ["received"], text: "alice", minAmount: 5 }, "oldestFirst", "milliDash", [], utcOffsetSecs(new Date(2026, 10, 1))]]);
    expect(saved).toEqual([["csv", "Dash Wallet history 2026-11-01.csv", btoa('"a"\n')]]);
  });

  it("answers null when the dialog is cancelled", async () => {
    const path = await exportHistory({ exportHistoryCsv: async () => "x", saveFileAs: async () => null }, "w1", { sort: "newestFirst", categories: [], text: null }, "dash", new Date());
    expect(path).toBeNull();
  });
});
