import { describe, expect, it } from "vitest";
import { navigate, switchWallet, ui } from "./ui";

describe("the masternode Activity filter", () => {
  it("ends on a wallet switch and on any navigation", () => {
    ui.set({ route: "activity", walletId: "a", activityFilter: { kind: "generated" } });
    switchWallet("b");
    expect(ui.get()).toMatchObject({ walletId: "b", activityFilter: null, detail: null });
    navigate("activity", { activityFilter: { kind: "txids", title: "t", txids: ["x"] } });
    expect(ui.get().activityFilter).toEqual({ kind: "txids", title: "t", txids: ["x"] });
    navigate("home");
    expect(ui.get().activityFilter).toBeNull();
  });
});
