// Tools ▸ Peers (QT-147, IOS-023): dash-qt's peer table over `peers()`, "—" where dash-spv reports
// nothing, and Change peers (`rotatePeers`). Disconnect and ban are node administration, which
// stays with the node (CLAUDE.md "Product scope"; U-05 D9).

import { hasCode, type Backend } from "../api/backend";
import type { PeerSummary } from "../api/generated/PeerSummary";
import { failureText, Model } from "../onboarding/model";
import { NONE } from "./information";

/** dash-qt `GUIUtil::formatBytes`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.floor(bytes / 1024)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${Math.floor(bytes / 1024 / 1024)} MB`;
  return `${Math.floor(bytes / 1024 / 1024 / 1024)} GB`;
}

/** dash-qt `GUIUtil::formatDurationStr`: "2 d 3 h 4 m 5 s", zero parts left out. */
export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const parts: string[] = [];
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) parts.push(`${d} d`);
  if (h) parts.push(`${h} h`);
  if (m) parts.push(`${m} m`);
  if (s % 60 || parts.length === 0) parts.push(`${s % 60} s`);
  return parts.join(" ");
}

export interface PeerRow {
  address: string;
  direction: "Inbound" | "Outbound";
  userAgent: string;
  version: string;
  height: string;
  ping: string;
  connectedFor: string;
  sent: string;
  received: string;
}

const or = <T,>(v: T | null, show: (v: T) => string) => (v === null ? NONE : show(v));

export function peerRow(p: PeerSummary, nowSecs: number): PeerRow {
  return {
    address: p.address,
    direction: p.inbound ? "Inbound" : "Outbound",
    userAgent: p.userAgent || NONE,
    version: or(p.protocolVersion, String),
    height: or(p.bestHeight, String),
    ping: or(p.pingMs, (ms) => `${ms} ms`),
    connectedFor: or(p.connectedSince, (t) => formatDuration(nowSecs - t)),
    sent: or(p.bytesSent, formatBytes),
    received: or(p.bytesReceived, formatBytes),
  };
}

export const PeersText = {
  none: "No peers are connected.",
  rotated: "Dropped every connection; connecting to other peers.",
  noPeers: "No other peers are known yet. Try again later.",
};

type Api = Pick<Backend, "peers" | "rotatePeers">;

export class PeersModel extends Model {
  peers: PeerSummary[] | null = null;
  error: string | null = null;
  /** What Change peers did. */
  notice: string | null = null;
  rotating = false;

  constructor(private readonly api: Api) {
    super();
  }

  async load() {
    try {
      this.peers = await this.api.peers();
      this.error = null;
    } catch (e) {
      this.error = `The peers could not be read: ${failureText(e)}`;
    }
    this.changed();
  }

  async rotate() {
    if (this.rotating) return;
    this.rotating = true;
    this.notice = null;
    this.changed();
    try {
      await this.api.rotatePeers();
      this.notice = PeersText.rotated;
    } catch (e) {
      this.notice = hasCode(e, "no_peers") ? PeersText.noPeers : failureText(e);
    }
    this.rotating = false;
    await this.load();
  }
}
