// Tools ▸ Information (QT-143, QT-144) and the node warning banner (QT-040): the Swift
// `InformationViewModel`, ported. dash-qt's General and Network rows; what only a full node has
// shows "—", the memory pool with "Requires full-node data source" (U-05 D4).

import type { Backend } from "../api/backend";
import { errorMessage } from "../api/backend";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { MasternodeCounts } from "../api/generated/MasternodeCounts";
import type { NodeSummary } from "../api/generated/NodeSummary";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { NodeWarning } from "../api/generated/NodeWarning";
import type { WarningCodeName } from "../api/generated/WarningCodeName";
import { Model } from "../onboarding/model";

export const NONE = "—";
export const REQUIRES_FULL_NODE = "Requires full-node data source";

export interface InfoRow {
  title: string;
  value: string;
  note?: string;
}

export interface InfoSection {
  title: string;
  rows: InfoRow[];
}

/** QT-040's banner texts. */
export const WARNING_TEXT: Record<WarningCodeName, string> = {
  prereleaseBuild: "This is a pre-release test build - use at your own risk - do not use for mining or merchant applications",
  uncleanShutdown: "The wallet was not shut down cleanly last time. It is checking its data.",
  syncStalled: "Sync has not made progress for a while. Try changing peers.",
  clockSkew: "Please check that your computer's date and time are correct! If your clock is wrong, Dash Wallet will not work properly.",
  platformContextUnavailable: "Dash Platform data cannot be verified right now.",
};

/** Most severe first: data being checked, then a clock that breaks everything, then sync. */
const SEVERITY: WarningCodeName[] = ["uncleanShutdown", "clockSkew", "syncStalled", "platformContextUnavailable", "prereleaseBuild"];

/** The banner shows the most severe warning; `null` with none. */
export function bannerText(warnings: NodeWarning[]): string | null {
  const worst = warnings.toSorted((a, b) => SEVERITY.indexOf(a.code) - SEVERITY.indexOf(b.code))[0];
  return worst ? WARNING_TEXT[worst.code] : null;
}

/** dash-qt's network names ("main", "test", "regtest", "devnet-<name>"). */
export function networkName(n: NetworkInfo): string {
  if (n.kind === "mainnet") return "main";
  if (n.kind === "testnet") return "test";
  return n.dirName;
}

/** dash-qt's `QDateTime::toString()`: "Wed Oct 14 20:40:00 2026". */
export function dateText(secs: number, timeZone?: string): string {
  const parts = new Intl.DateTimeFormat("en-US", {
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    year: "numeric",
    hourCycle: "h23",
    timeZone,
  }).formatToParts(new Date(secs * 1000));
  const p = Object.fromEntries(parts.map((x) => [x.type, x.value]));
  return `${p.weekday} ${p.month} ${p.day} ${p.hour}:${p.minute}:${p.second} ${p.year}`;
}

/** dash-qt `formatBytes`-style megabytes with one decimal. */
export const megabytes = (bytes: number) => `${(bytes / 1_000_000).toFixed(1)} MB`;

const counts = (c: MasternodeCounts | null) => (c ? `Total: ${c.total} (Enabled: ${c.enabled})` : NONE);

export function informationSections(launch: LaunchInfo, info: NodeSummary, timeZone?: string): InfoSection[] {
  const fullNode = (value: number | null, show: (v: number) => string): string | [string, string] =>
    value === null ? [NONE, REQUIRES_FULL_NODE] : show(value);
  const row = (title: string, value: string | [string, string]): InfoRow =>
    typeof value === "string" ? { title, value } : { title, value: value[0], note: value[1] };
  const lock = info.bestChainlock;
  return [
    {
      title: "General",
      rows: [
        row("Client version", launch.appVersion),
        row("Engine version", launch.engineVersion),
        row("User Agent", info.userAgent),
        row("Datadir", info.dataDir),
        row("Startup time", dateText(info.startupTime, timeZone)),
      ],
    },
    {
      title: "Network",
      rows: [
        row("Name", networkName(info.network)),
        row("Number of connections", `${info.connectionsIn + info.connectionsOut} (In: ${info.connectionsIn} / Out: ${info.connectionsOut})`),
        row("Local Addresses", info.localAddresses.length ? info.localAddresses.join(", ") : NONE),
      ],
    },
    {
      title: "Block chain",
      rows: [
        row("Current block height", info.tipHeight === null ? NONE : String(info.tipHeight)),
        row("Last block time", info.tipTime === null ? NONE : dateText(info.tipTime, timeZone)),
        row("Last block hash", info.tipHash ?? NONE),
      ],
    },
    {
      title: "Memory Pool",
      rows: [
        row("Current number of transactions", fullNode(info.mempoolTxCount, String)),
        row("Memory usage", fullNode(info.mempoolUsageBytes, megabytes)),
      ],
    },
    {
      title: "Masternodes",
      rows: [
        row("Masternodes", counts(info.masternodes)),
        row("EvoNodes", counts(info.evonodes)),
        row(
          "ChainLocks",
          lock ? [String(lock.height), lock.blockTime === null ? null : dateText(lock.blockTime, timeZone), lock.blockHash].filter((x) => x !== null).join(", ") : NONE,
        ),
      ],
    },
  ];
}

type Api = Pick<Backend, "nodeInfo" | "warnings">;

/** Reads both on show and again on every sync change (the screen calls `load`). */
export class InformationModel extends Model {
  info: NodeSummary | null = null;
  warnings: NodeWarning[] = [];
  error: string | null = null;
  #seq = 0;

  constructor(private readonly api: Api) {
    super();
  }

  get banner(): string | null {
    return bannerText(this.warnings);
  }

  async load() {
    const seq = ++this.#seq;
    try {
      const [info, warnings] = await Promise.all([this.api.nodeInfo(), this.api.warnings()]);
      if (seq !== this.#seq) return;
      this.info = info;
      this.warnings = warnings;
      this.error = null;
    } catch (e) {
      if (seq !== this.#seq) return;
      this.error = `The node information could not be read: ${errorMessage(e)}`;
    }
    this.changed();
  }
}
