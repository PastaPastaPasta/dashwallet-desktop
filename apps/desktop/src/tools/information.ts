// Tools ▸ Information (QT-143, QT-144) and the node warning banner (QT-040): the Swift
// `InformationViewModel`, ported. dash-qt's General and Network rows; what only a full node has
// shows "—", the memory pool with "Requires full-node data source" (U-05 D4).

import type { Backend } from "../api/backend";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { MasternodeCounts } from "../api/generated/MasternodeCounts";
import type { NodeSummary } from "../api/generated/NodeSummary";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { NodeWarning } from "../api/generated/NodeWarning";
import type { WarningCodeName } from "../api/generated/WarningCodeName";
import { messageTable, t } from "../i18n";
import { failureText, Model } from "../onboarding/model";

export const NONE = "—";

export interface InfoRow {
  title: string;
  value: string;
  note?: string;
  /** A version string: shown left to right in every language. */
  ltr?: true;
}

export interface InfoSection {
  id: string;
  title: string;
  rows: InfoRow[];
}

/** QT-040's banner texts. */
export const WARNING_TEXT: Readonly<Record<WarningCodeName, string>> = messageTable("tools.information.warnings", [
  "prereleaseBuild",
  "uncleanShutdown",
  "syncStalled",
  "clockSkew",
  "platformContextUnavailable",
]);

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

const counts = (c: MasternodeCounts | null) =>
  c ? t("tools.information.masternodeCounts", { total: String(c.total), enabled: String(c.enabled) }) : NONE;

export function informationSections(launch: LaunchInfo, info: NodeSummary, timeZone?: string): InfoSection[] {
  const fullNode = (value: number | null, show: (v: number) => string): string | [string, string] =>
    value === null ? [NONE, t("tools.information.requiresFullNode")] : show(value);
  const row = (title: string, value: string | [string, string]): InfoRow =>
    typeof value === "string" ? { title, value } : { title, value: value[0], note: value[1] };
  const lock = info.bestChainlock;
  return [
    {
      id: "general",
      title: t("tools.information.sections.general"),
      rows: [
        { ...row(t("tools.information.rows.clientVersion"), launch.buildVersion), ltr: true },
        { ...row(t("tools.information.rows.engineVersion"), launch.engineVersion), ltr: true },
        row(t("tools.information.rows.userAgent"), info.userAgent),
        row(t("tools.information.rows.dataDir"), info.dataDir),
        row(t("tools.information.rows.startupTime"), dateText(info.startupTime, timeZone)),
      ],
    },
    {
      id: "network",
      title: t("tools.information.sections.network"),
      rows: [
        row(t("tools.information.rows.networkName"), networkName(info.network)),
        row(
          t("tools.information.rows.connections"),
          t("tools.information.connectionsValue", {
            total: String(info.connectionsIn + info.connectionsOut),
            inCount: String(info.connectionsIn),
            outCount: String(info.connectionsOut),
          }),
        ),
        row(t("tools.information.rows.localAddresses"), info.localAddresses.length ? info.localAddresses.join(", ") : NONE),
      ],
    },
    {
      id: "block-chain",
      title: t("tools.information.sections.blockChain"),
      rows: [
        row(t("tools.information.rows.blockHeight"), info.tipHeight === null ? NONE : String(info.tipHeight)),
        row(t("tools.information.rows.lastBlockTime"), info.tipTime === null ? NONE : dateText(info.tipTime, timeZone)),
        row(t("tools.information.rows.lastBlockHash"), info.tipHash ?? NONE),
      ],
    },
    {
      id: "memory-pool",
      title: t("tools.information.sections.memoryPool"),
      rows: [
        row(t("tools.information.rows.mempoolTxCount"), fullNode(info.mempoolTxCount, String)),
        row(t("tools.information.rows.mempoolUsage"), fullNode(info.mempoolUsageBytes, megabytes)),
      ],
    },
    {
      id: "masternodes",
      title: t("tools.information.sections.masternodes"),
      rows: [
        row(t("tools.information.rows.masternodes"), counts(info.masternodes)),
        row(t("tools.information.rows.evonodes"), counts(info.evonodes)),
        row(
          t("tools.information.rows.chainLocks"),
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
      this.error = t("tools.information.loadFailed", { error: failureText(e) });
    }
    this.changed();
  }
}
