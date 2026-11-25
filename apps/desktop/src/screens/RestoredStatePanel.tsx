import { useId } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { errorMessage, isAppError, type Backend } from "../api/backend";
import type { LockStatus } from "../api/generated/LockStatus";
import { keys, useBackend, useRestored, useRestoreStatus, useWallets } from "../api/queries";
import { setRestoreFinishing } from "../api/restoreGate";
import type { RestoreStatus } from "../api/generated/RestoreStatus";
import type { RestoreWait } from "../api/generated/RestoreWait";
import { DashButton } from "../components/basics";
import { t, useT } from "../i18n";
import { busyLine, type BusyLines } from "../network/choice";
import { bootAgain, closeRefusal, type CloseRefusalText } from "../network/switch";
import { Model, useModel } from "../onboarding/model";
import { FailedLink, failedLinkUrl } from "./settings/About";
import { Problem } from "./settings/parts";

/** "wallet" (no wallet loaded yet) has no line: the panel can't be reached without a wallet. */
const WAITING = {
  coreSync: "storeUpgrade.restore.waiting.core",
  platformPass: "storeUpgrade.restore.waiting.platform",
  historyLoad: "storeUpgrade.restore.waiting.history",
  wallet: null,
} as const satisfies Record<RestoreWait, string | null>;

const saidWaits = (s: RestoreStatus) =>
  s.waiting.flatMap((wait) => {
    const line = WAITING[wait];
    return line === null ? [] : [{ wait, line }];
  });

/** Try again while the engine waits on something or couldn't write the clear; a history that failed
 * to load offers Report a problem only. */
export const offersRetry = (s: RestoreStatus) => saidWaits(s).length > 0 || s.clearFailed;

/** Only while Platform's first check is what it waits for (with Platform off it never is), a wallet is
 * loaded, and no history failed to load: RESTORED can't clear without it, with or without Platform.
 * Otherwise the engine returns without reopening. */
export const offersFinishWithoutPlatform = (s: RestoreStatus) =>
  s.waiting.includes("platformPass") && !s.waiting.includes("wallet") && s.historyWallets.length === 0 && !s.platformOnly;

const FINISH_BUSY: BusyLines = {
  wait: (flows) => t("storeUpgrade.restore.finishBusy.waitBefore", { flows }),
  waitOrStopMixing: (flows) => t("storeUpgrade.restore.finishBusy.waitOrStopMixing", { flows }),
  waitLockStops: (flows, stops) => t("storeUpgrade.restore.finishBusy.waitBeforeLockStops", { flows, stops }),
  waitOrStopMixingLockStops: (flows, stops) => t("storeUpgrade.restore.finishBusy.waitOrStopMixingLockStops", { flows, stops }),
  stopMixing: () => t("storeUpgrade.restore.finishBusy.stopMixing"),
};

/** Finish without Dash Platform's refusals, as the switch's but in its own words; a refused close
 * says the engine's error (MG). */
const FINISH_REFUSAL: CloseRefusalText = {
  busy: (waits, mixing, lockStops) => busyLine(FINISH_BUSY, waits, mixing, lockStops),
  get busyDraining() {
    return t("storeUpgrade.restore.finishBusy.busyDraining");
  },
  walletOpening: null,
  shieldedUnresolved: null,
};

const saysSomething = (s: RestoreStatus) => saidWaits(s).length > 0 || s.historyWallets.length > 0 || s.clearFailed || s.platformOnly;

/** The panel's actions through dw-app, with the latest failure. Finish without Dash Platform is one
 * engine call: a close refused while the network is busy changes nothing and says why here; an open
 * without Platform that fails opens the network as usual, and the panel says so; otherwise the page
 * boots again on the new session. While it runs, the money gate reads RESTORED as unknown. */
export class RestoredStateActions extends Model {
  running = false;
  problem: string | null = null;
  failedLink: string | null = null;
  /** What the panel showed when Finish without Dash Platform started: shown until it ends. */
  finishing: RestoreStatus | null = null;

  constructor(
    private readonly api: Pick<Backend, "restoreRetry" | "finishWithoutPlatform" | "openProjectLink">,
    private readonly client: QueryClient,
    private readonly boot: (note: string | null, withoutPlatform?: boolean) => unknown = (note, withoutPlatform) => bootAgain(note, undefined, withoutPlatform),
  ) {
    super();
  }

  retry = async () => {
    if (!(await this.#run(() => this.api.restoreRetry()))) return;
    void this.client.invalidateQueries({ queryKey: keys.restoreStatus });
    void this.client.invalidateQueries({ queryKey: keys.restored });
  };

  finishWithoutPlatform = async (shown: RestoreStatus) => {
    if (this.running) return;
    this.#start();
    this.finishing = shown;
    setRestoreFinishing(true);
    this.changed();
    try {
      await this.api.finishWithoutPlatform();
    } catch (e) {
      const failed = isAppError(e) && e.code === "app.network_switch_failed" && e.data?.kind === "networkSwitch" ? e.data : null;
      if (failed && !failed.rolledBack) return void (await this.boot(null));
      if (failed) await this.#readAgain();
      const canLock = this.client.getQueryData<LockStatus>(keys.lockStatus)?.encrypted === true;
      this.problem = failed ? t("storeUpgrade.restore.finishFailed", { error: errorMessage(e) }) : (closeRefusal(e, canLock, FINISH_REFUSAL) ?? errorMessage(e));
      this.running = false;
      this.finishing = null;
      setRestoreFinishing(false);
      this.changed();
      return;
    }
    await this.boot(null, true);
  };

  /** Opened as usual: a new session, so every read is read again. RESTORED, what it waits for and the
   * lock are read before the gate trusts RESTORED again; a RESTORED that can't be read is not known. */
  async #readAgain() {
    const first = [keys.restored, keys.restoreStatus, keys.lockStatus];
    const heads: unknown[] = [keys.network[0], ...first.map((k) => k[0])];
    void this.client.invalidateQueries({ predicate: (q) => !heads.includes(q.queryKey[0]) });
    await Promise.all(first.map((queryKey) => this.client.invalidateQueries({ queryKey })));
    if (this.client.getQueryState(keys.restored)?.status === "error") this.client.removeQueries({ queryKey: keys.restored });
  }

  report = async () => {
    await this.#run(
      () => this.api.openProjectLink("issues"),
      (e) => (this.failedLink = failedLinkUrl(e)) !== null,
    );
  };

  #start() {
    this.problem = null;
    this.failedLink = null;
    this.running = true;
  }

  /** Runs `call`; `quiet` takes a failure it shows itself. */
  async #run(call: () => Promise<unknown>, quiet?: (e: unknown) => boolean): Promise<boolean> {
    this.#start();
    this.changed();
    try {
      await call();
      return true;
    } catch (e) {
      if (!quiet?.(e)) this.problem = errorMessage(e);
      return false;
    } finally {
      this.running = false;
      this.changed();
    }
  }
}

/** One per app: what the panel says outlives the panel, so a failed finish is still said after the
 * lock screen a reopen may bring, and the panel stays while the session closes under it. */
const shared = new WeakMap<QueryClient, RestoredStateActions>();

export function restoredStateActions(api: Backend, client: QueryClient): RestoredStateActions {
  let actions = shared.get(client);
  if (!actions) shared.set(client, (actions = new RestoredStateActions(api, client)));
  return actions;
}

/** Under the restored banner while `restore_status` says something (RESTORED set, or only Platform and
 * Shielded funds held): what the engine still waits for, Try again, Finish without Dash Platform while
 * Platform's first check holds the Core funds too, and Report a problem. Read again on each sync
 * (events.ts), never on a timer; nothing while RESTORED is not known. */
export function RestoredStatePanel() {
  const restored = useRestored().data;
  const known = restored !== undefined;
  const status = useRestoreStatus(known);
  const wallets = useWallets(known);
  const actions = useModel(restoredStateActions(useBackend(), useQueryClient()));
  const held = restored === true || (restored === false && status.data?.platformOnly === true);
  const shown = actions.finishing ?? (held ? status.data : null);
  if (!shown) return null;
  const names = new Map((wallets.data ?? []).map((w) => [w.walletId, w.name]));
  return <RestoredStateView status={shown} nameOf={(id) => names.get(id) ?? `${id.slice(0, 8)}…`} actions={actions} />;
}

export function RestoredStateView({
  status,
  nameOf,
  actions,
}: {
  status: RestoreStatus;
  nameOf: (walletId: string) => string;
  actions: Pick<RestoredStateActions, "running" | "problem" | "failedLink" | "retry" | "finishWithoutPlatform" | "report">;
}) {
  const tr = useT();
  const meaningId = useId();
  if (!saysSomething(status)) return null;
  const finish = offersFinishWithoutPlatform(status);
  return (
    <section className="restored-state-panel card-l1" data-testid="restored-state" aria-busy={actions.running}>
      <div className="protocol-advisory-body" role="status">
        {status.platformOnly && (
          <p className="protocol-advisory-text" data-testid="restored-platform-only">
            {tr("storeUpgrade.restore.platformOnly")}
          </p>
        )}
        {saidWaits(status).map(({ wait, line }) => (
          <p key={wait} className="protocol-advisory-text" data-testid={`restored-waiting-${wait}`}>
            {tr(line)}
          </p>
        ))}
        {status.historyWallets.map((walletId) => (
          <p key={walletId} className="protocol-advisory-text" data-testid="restored-history-failed">
            {tr("storeUpgrade.restore.historyFailed", { wallet: nameOf(walletId) })}
          </p>
        ))}
        {status.clearFailed && (
          <p className="protocol-advisory-text" data-testid="restored-clear-failed">
            {tr("storeUpgrade.restore.clearFailed")}
          </p>
        )}
      </div>
      <div className="history-gap-actions">
        {offersRetry(status) && (
          <DashButton variant="tintedBlue" size="small" data-testid="restored-retry" disabled={actions.running} onClick={() => void actions.retry()}>
            {tr("storeUpgrade.restore.retry")}
          </DashButton>
        )}
        {finish && (
          <DashButton
            variant="plainBlue"
            size="small"
            data-testid="restored-finish-without-platform"
            aria-describedby={meaningId}
            disabled={actions.running}
            onClick={() => void actions.finishWithoutPlatform(status)}
          >
            {tr("storeUpgrade.restore.finishWithoutPlatform")}
          </DashButton>
        )}
        <DashButton variant="plainBlue" size="small" data-testid="restored-report" disabled={actions.running} onClick={() => void actions.report()}>
          {tr("settings.about.links.issues")}
        </DashButton>
      </div>
      {finish && (
        <p id={meaningId} className="field-hint" data-testid="restored-finish-meaning">
          {tr("storeUpgrade.restore.finishWithoutPlatformMeaning")}
        </p>
      )}
      <Problem text={actions.problem} testId="restored-state-problem" />
      {actions.failedLink && (
        <p className="form-problem" role="alert" data-testid="restored-state-link-failed">
          <FailedLink url={actions.failedLink} />
        </p>
      )}
    </section>
  );
}
