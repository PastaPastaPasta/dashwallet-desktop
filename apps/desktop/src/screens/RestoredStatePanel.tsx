import { useId } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { errorMessage, type Backend } from "../api/backend";
import type { OpenedNetwork } from "../api/generated/OpenedNetwork";
import { lockGeneration, seedLockStatus } from "../api/lockGate";
import { keys, useBackend, useRestored, useRestoreStatus, useWallets } from "../api/queries";
import type { RestoreStatus, RestoreWait } from "../api/restoreHolds";
import { isRestored } from "../api/storeOpen";
import { DashButton } from "../components/basics";
import { t, useT } from "../i18n";
import { bootAgain } from "../network/switch";
import { Model, useModel } from "../onboarding/model";
import { FailedLink, failedLinkUrl } from "./settings/About";
import { Problem } from "./settings/parts";

const WAITING = {
  coreSync: "storeUpgrade.restore.waiting.core",
  platformPass: "storeUpgrade.restore.waiting.platform",
  historyLoad: "storeUpgrade.restore.waiting.history",
} as const satisfies Record<RestoreWait, string>;

/** Try again while the engine waits on something or couldn't write the clear; a history that failed
 * to load offers Report a problem only. */
export const offersRetry = (s: RestoreStatus) => s.waiting.length > 0 || s.clearFailed;

/** Only while Platform's first check is what it waits for (with Platform off it never is), and no
 * history failed to load: RESTORED can't clear without it, with or without Platform. */
export const offersFinishWithoutPlatform = (s: RestoreStatus) => s.waiting.includes("platformPass") && s.historyWallets.length === 0;

const saysSomething = (s: RestoreStatus) => s.waiting.length > 0 || s.historyWallets.length > 0 || s.clearFailed;

/** The panel's actions through dw-app, with the latest failure. Finish without Dash Platform closes
 * the network, opens it again with `no_platform` and boots the page again on that session. A close
 * that is refused changes nothing and says why here; an open without Platform that fails opens the
 * network as usual, and the panel says so. */
export class RestoredStateActions extends Model {
  running = false;
  problem: string | null = null;
  failedLink: string | null = null;

  constructor(
    private readonly api: Pick<Backend, "restoreRetry" | "closeNetwork" | "openNetworkWithoutPlatform" | "openNetwork" | "openProjectLink">,
    private readonly client: QueryClient,
    private readonly boot: (note: string | null) => unknown = bootAgain,
  ) {
    super();
  }

  retry = async () => {
    if (!(await this.#run(() => this.api.restoreRetry()))) return;
    void this.client.invalidateQueries({ queryKey: keys.restoreStatus });
    void this.client.invalidateQueries({ queryKey: keys.restored });
  };

  finishWithoutPlatform = async () => {
    if (!(await this.#run(() => this.api.closeNetwork(), true))) return;
    try {
      await this.api.openNetworkWithoutPlatform();
    } catch (e) {
      return this.#reopen(e);
    }
    await this.boot(null);
  };

  /** Opens the network as usual after the open without Platform failed: a new session, so every read
   * is read again. When that fails too, the page boots again and its own open says why. */
  async #reopen(failure: unknown) {
    const since = lockGeneration();
    let opened: OpenedNetwork;
    try {
      opened = await this.api.openNetwork();
    } catch {
      return void (await this.boot(null));
    }
    seedLockStatus(this.client, opened.lock, since);
    this.client.setQueryData(keys.restored, isRestored(opened.network));
    void this.client.invalidateQueries({ predicate: (q) => q.queryKey[0] !== keys.network[0] && q.queryKey[0] !== keys.lockStatus[0] });
    this.problem = t("storeUpgrade.restore.finishFailed", { error: errorMessage(failure) });
    this.running = false;
    this.changed();
  }

  report = async () => {
    await this.#run(
      () => this.api.openProjectLink("issues"),
      false,
      (e) => (this.failedLink = failedLinkUrl(e)) !== null,
    );
  };

  /** Runs `call`; `keepRunning` leaves the buttons disabled after it worked (the page boots again);
   * `quiet` takes a failure it shows itself. */
  async #run(call: () => Promise<unknown>, keepRunning = false, quiet?: (e: unknown) => boolean): Promise<boolean> {
    this.problem = null;
    this.failedLink = null;
    this.running = true;
    this.changed();
    try {
      await call();
      this.running = keepRunning;
      return true;
    } catch (e) {
      if (!quiet?.(e)) this.problem = errorMessage(e);
      this.running = false;
      return false;
    } finally {
      this.changed();
    }
  }
}

/** One per app: what the panel says outlives the panel, so a failed finish is still said after the
 * lock screen a reopen may bring. */
const shared = new WeakMap<QueryClient, RestoredStateActions>();

export function restoredStateActions(api: Backend, client: QueryClient): RestoredStateActions {
  let actions = shared.get(client);
  if (!actions) shared.set(client, (actions = new RestoredStateActions(api, client)));
  return actions;
}

/** Under the restored banner while RESTORED is set: what the engine still waits for, Try again, Finish
 * without Dash Platform while Platform's first check is the wait, and Report a problem. Read again on
 * each sync (events.ts), never on a timer. */
export function RestoredStatePanel() {
  const restored = useRestored().data === true;
  const status = useRestoreStatus(restored);
  const wallets = useWallets(restored);
  const actions = useModel(restoredStateActions(useBackend(), useQueryClient()));
  if (!restored || !status.data) return null;
  const names = new Map((wallets.data ?? []).map((w) => [w.walletId, w.name]));
  return <RestoredStateView status={status.data} nameOf={(id) => names.get(id) ?? `${id.slice(0, 8)}…`} actions={actions} />;
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
    <section className="restored-state-panel card-l1" data-testid="restored-state">
      <div className="protocol-advisory-body" role="status">
        {status.waiting.map((wait) => (
          <p key={wait} className="protocol-advisory-text" data-testid={`restored-waiting-${wait}`}>
            {tr(WAITING[wait])}
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
            onClick={() => void actions.finishWithoutPlatform()}
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
