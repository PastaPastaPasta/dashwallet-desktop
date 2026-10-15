// Tools ▸ Information and Peers, by parity ID (the Swift `ToolsViewModelTests`, ported).

import { describe, expect, it, vi } from "vitest";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { NodeSummary } from "../api/generated/NodeSummary";
import type { PeerSummary } from "../api/generated/PeerSummary";
import { appError } from "../onboarding/testWorld";
import { bannerText, dateText, informationSections, InformationModel, NONE, REQUIRES_FULL_NODE } from "./information";
import { formatBytes, formatDuration, peerRow, PeersModel, PeersText } from "./peers";

const testnet = { kind: "testnet", dirName: "testnet", unitName: "tDASH" } as const;

export const LAUNCH: LaunchInfo = {
  selftest: false,
  selftestPauseMs: 0,
  selftestUnlock: false,
  selftestLinkTrace: false,
  theme: null,
  network: testnet,
  appVersion: "0.1.0",
  engineVersion: "0.4.2",
};

/** The Swift `FakeNodeInformation.sample()`. */
export const NODE: NodeSummary = {
  userAgent: "/dash-spv:0.42.0/",
  dataDir: "/home/user/.local/share/dash-wallet/testnet",
  startupTime: 1_760_000_000,
  network: testnet,
  connectionsIn: 0,
  connectionsOut: 8,
  localAddresses: [],
  tipHeight: 1_200_000,
  tipTime: 1_760_000_100,
  tipHash: "00".repeat(32),
  bestChainlock: { height: 1_199_998, blockHash: "11".repeat(32), blockTime: null },
  masternodes: { total: 3000, enabled: 2900 },
  evonodes: null,
  mempoolTxCount: null,
  mempoolUsageBytes: null,
};

const rows = (info: NodeSummary) => informationSections(LAUNCH, info, "UTC").flatMap((s) => s.rows);
const row = (info: NodeSummary, title: string) => rows(info).find((r) => r.title === title);

describe("Information (QT-143, QT-144, QT-040)", () => {
  it("QT143_unknownAndFullNodeOnlyRowsShowADash", () => {
    expect(row(NODE, "Number of connections")?.value).toBe("8 (In: 0 / Out: 8)");
    expect(row(NODE, "Name")?.value).toBe("test");
    expect(row(NODE, "Current number of transactions")).toEqual({ title: "Current number of transactions", value: NONE, note: REQUIRES_FULL_NODE });
    expect(row(NODE, "Memory usage")?.note).toBe(REQUIRES_FULL_NODE);
    expect(row(NODE, "Masternodes")?.value).toBe("Total: 3000 (Enabled: 2900)");
    expect(row(NODE, "EvoNodes")?.value).toBe(NONE);
    expect(row(NODE, "Local Addresses")?.value).toBe(NONE);
    expect(row(NODE, "ChainLocks")?.value).toBe(`1199998, ${"11".repeat(32)}`);
    expect(row(NODE, "Client version")?.value).toBe("0.1.0");
  });

  it("QT144_fullNodeValuesShowWhenTheEngineHasThem", () => {
    const full = { ...NODE, mempoolTxCount: 12, mempoolUsageBytes: 2_345_678, network: { kind: "devnet", dirName: "devnet-paloma", unitName: "tDASH" } } as const;
    expect(row(full, "Current number of transactions")).toEqual({ title: "Current number of transactions", value: "12" });
    expect(row(full, "Memory usage")?.value).toBe("2.3 MB");
    expect(row(full, "Name")?.value).toBe("devnet-paloma");
  });

  it("formats dates as dash-qt", () => {
    expect(dateText(1_760_000_000, "UTC")).toBe("Thu Oct 9 08:53:20 2025");
    expect(row(NODE, "Startup time")?.value).toBe("Thu Oct 9 08:53:20 2025");
  });

  it("QT040_bannerShowsTheMostSevereWarning", async () => {
    const warnings = [
      { code: "syncStalled", detail: "" },
      { code: "clockSkew", detail: "" },
    ] as const;
    expect(bannerText([...warnings])).toMatch(/^Please check that your computer's date and time are correct!/);
    expect(bannerText([])).toBeNull();
    const model = new InformationModel({ nodeInfo: async () => NODE, warnings: async () => [...warnings] });
    await model.load();
    expect(model.banner).toBe(bannerText([...warnings]));
    expect(model.info).toBe(NODE);
  });

  it("a failed read keeps the last facts and says so", async () => {
    const nodeInfo = vi.fn<() => Promise<NodeSummary>>().mockResolvedValueOnce(NODE).mockRejectedValueOnce(appError("network_not_open"));
    const model = new InformationModel({ nodeInfo, warnings: async () => [] });
    await model.load();
    await model.load();
    expect(model.info).toBe(NODE);
    expect(model.error).toMatch(/could not be read/);
  });
});

const PEER: PeerSummary = {
  address: "203.0.113.1:19999",
  userAgent: null,
  protocolVersion: null,
  bestHeight: null,
  pingMs: null,
  connectedSince: null,
  inbound: false,
  bytesSent: null,
  bytesReceived: null,
};

describe("Peers (QT-147, IOS-023)", () => {
  it("QT147_rowsShowADashWhereDashSpvReportsNothing", () => {
    expect(peerRow(PEER, 0)).toEqual({
      address: "203.0.113.1:19999",
      direction: "Outbound",
      userAgent: NONE,
      version: NONE,
      height: NONE,
      ping: NONE,
      connectedFor: NONE,
      sent: NONE,
      received: NONE,
    });
    const full = { ...PEER, userAgent: "/Dash Core:23.1.0/", protocolVersion: 70235, bestHeight: 1_200_000, pingMs: 42, connectedSince: 1000, inbound: true, bytesSent: 2048, bytesReceived: 5 * 1024 * 1024 };
    expect(peerRow(full, 1000 + 93_784)).toMatchObject({ direction: "Inbound", version: "70235", ping: "42 ms", connectedFor: "1 d 2 h 3 m 4 s", sent: "2 KB", received: "5 MB" });
  });

  it("formats like dash-qt", () => {
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatDuration(0)).toBe("0 s");
    expect(formatDuration(3600)).toBe("1 h");
  });

  it("IOS023_changePeersDropsEveryConnectionAndReloads", async () => {
    const api = { peers: vi.fn(async () => [PEER]), rotatePeers: vi.fn(async () => null) };
    const model = new PeersModel(api);
    await model.rotate();
    expect(api.rotatePeers).toHaveBeenCalledOnce();
    expect(model.notice).toBe(PeersText.rotated);
    expect(model.peers).toEqual([PEER]);
    api.rotatePeers.mockRejectedValueOnce(appError("no_peers"));
    await model.rotate();
    expect(model.notice).toBe(PeersText.noPeers);
  });
});
