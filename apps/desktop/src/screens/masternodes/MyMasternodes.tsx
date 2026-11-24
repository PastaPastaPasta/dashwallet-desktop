// Masternodes ▸ My masternodes (MG-10 §1.2): the summary, the groups with In progress (S2), the
// pane with Release collateral / Lock it again (S2), Change keys or payout (S3) and the operator
// explanation sheet, and the page's states. Tracking, keys and withdraw come with other slices.

import { Fragment, useState } from "react";
import type { MasternodeDetail } from "../../api/generated/MasternodeDetail";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { navigate, ui } from "../../app/ui";
import { AmountText, DashButton } from "../../components/basics";
import { CopyIconButton } from "../../components/copy";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { copyWithToast } from "../../components/toast";
import { useDisplay, type Display } from "../../format/display";
import { shortAddress, shortDate, shortHash } from "../../governance/model";
import {
  activityChip,
  GROUP_TITLE,
  groups,
  heldRoles,
  isEmpty,
  kindText,
  lastPayoutTitle,
  MnText,
  mnErrorText,
  operatorShare,
  payoutGroups,
  registrarLine,
  rowMeta,
  rowName,
  rowSub,
  sharesPayoutAddress,
  statusChip,
  summary,
  voteBlocked,
} from "../../masternodes/model";
import { useMasternode, useMasternodePayouts, useMasternodes, useMasternodesRefresh, useMasternodeOperations } from "../../masternodes/queries";
import { conflicted, inProgress } from "../../masternodes/operations";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import { InProgressGroup, useInProgressActions, type InProgressActions } from "./InProgress";
import { registrationCalls, registrationSends } from "../../masternodes/registration";
import { collateralErrorText, ReleaseSheet } from "./ReleaseSheet";
import { useT } from "../../i18n";
import { ChangeKeysSheet, OperatorSheet } from "./ChangeKeysSheet";
import { MoreMenu, Notice, StatusChip } from "./common";

const DAY = 86_400;
/** mn-engine's `MAX_MASTERNODE_LABEL`. */
const MAX_LABEL = 64;

interface MyMasternodesProps {
  wallet: WalletSummary;
  network: NetworkInfo;
  onVote: () => void;
  onContinue: (op: ProTxOperation) => void;
}

/** In progress's open sheet lives here, beside the list's layouts: a list read that switches them
 * (failed to loaded, or back) keeps it open. */
export function MyMasternodes(props: MyMasternodesProps) {
  const actions = useInProgressActions(props.wallet, props.network, props.onContinue);
  return (
    <>
      <MyMasternodesBody {...props} actions={actions} />
      {actions.sheets}
    </>
  );
}

function MyMasternodesBody({ wallet, network, onVote, actions }: MyMasternodesProps & { actions: InProgressActions }) {
  const list = useMasternodes(wallet.walletId);
  const ops = useMasternodeOperations(wallet.walletId);
  const refresh = useMasternodesRefresh();
  const t = useT();
  const shown = useDisplay(network);
  const [selected, setSelected] = useState<string | null>(null);
  const [retiredOpen, setRetiredOpen] = useState(false);

  const work = ops.data ?? [];
  // A failed or pending read is not an empty one: saved work stays named until the read answers.
  const progress = (
    <>
      {ops.isError && (
        <Notice
          tone="warning"
          testId="mn-ops-failed"
          action={
            <DashButton variant="filledBlue" size="small" data-testid="mn-ops-retry" onClick={() => void ops.refetch()}>
              {MnText.tryAgain}
            </DashButton>
          }
        >
          <strong>{MnText.opsFailed}</strong> {mnErrorText(ops.error)}
        </Notice>
      )}
      {ops.isPending ? <LoadingState testId="mn-ops-loading" text={MnText.opsLoading} /> : <InProgressGroup ops={work} wallet={wallet} network={network} actions={actions} />}
    </>
  );
  if (list.isPending) return <LoadingState testId="mn-loading" text={MnText.loading} />;
  if (list.isError)
    return (
      <>
        <Notice
          tone="warning"
          testId="mn-list-failed"
          action={
            <DashButton variant="filledBlue" size="small" data-testid="mn-retry" onClick={() => void refresh()}>
              {MnText.tryAgain}
            </DashButton>
          }
        >
          <strong>{MnText.failed}</strong> {mnErrorText(list.error)}
        </Notice>
        {progress}
      </>
    );
  const o = list.data;
  const watchNotice = wallet.watchOnly && (
    <Notice tone="info" testId="mn-notice-watch-only">
      <strong>{MnText.watchOnly}</strong> {MnText.watchOnlyText}
    </Notice>
  );
  if (isEmpty(o) && inProgress(work).length === 0 && conflicted(work).length === 0)
    return (
      <>
        {watchNotice}
        {!ops.isSuccess && progress}
        <section className="mg-empty card-l1" data-testid="mn-empty">
          <h2>{MnText.emptyTitle}</h2>
          <p>{MnText.emptyText}</p>
          {o.collateralCoins.length > 0 && <p data-testid="mn-empty-coin">{MnText.emptyCoin}</p>}
          <ul className="mg-empty-points">
            {MnText.emptyPoints.map(([title, text]) => (
              <li key={title}>
                <strong>{title}</strong>
                <span>{text}</span>
              </li>
            ))}
          </ul>
        </section>
      </>
    );

  const synced = o.list.synced;
  const all = groups(o.masternodes);
  const visible = all.flatMap((g) => (g.id === "retired" && !retiredOpen ? [] : g.rows));
  const current = visible.find((r) => r.proTxHash === selected) ?? visible[0] ?? null;
  const s = summary(o);
  const amount = (duffs: number) => <AmountText text={shown.compact(duffs)} unitName={shown.name} />;
  return (
    <>
      {watchNotice}
      {!synced && <LoadingState testId="mn-syncing" text={MnText.loading} />}
      <div className="mg-summary card-l1" data-testid="mn-summary">
        <div>
          <span className="mg-summary-label">{t("masternodes.myMasternodes.summary.label")}</span>
          <span className="mg-summary-value">{t.rich("masternodes.myMasternodes.summary.yours", { count: s.count, muted: (c) => <span className="mg-muted mg-small">{c}</span> })}</span>
          <span className="mg-summary-note">{s.countNote}</span>
        </div>
        <div data-testid="mn-summary-payouts">
          <span className="mg-summary-label">{t("masternodes.myMasternodes.summary.payoutsLabel")}</span>
          <span className="mg-summary-value">{amount(s.payouts)}</span>
          <span className="mg-summary-note">{t("masternodes.myMasternodes.summary.payoutsNote")}</span>
          {s.unattributed !== null && (
            <>
              <span className="mg-summary-value" data-testid="mn-summary-unattributed">
                {amount(s.unattributed)}
              </span>
              <span className="mg-summary-note">{t("masternodes.myMasternodes.summary.unattributedNote", { note: s.unattributedNote ?? "" })}</span>
              <DashButton variant="plainBlue" size="small" data-testid="mn-summary-activity" onClick={() => navigate("activity", { activityFilter: { kind: "generated" } })}>
                {MnText.seeInActivity}
              </DashButton>
            </>
          )}
        </div>
      </div>
      <div className="contacts-split">
        <div className="contacts-list">
          {all.flatMap((g, i) => [
            g.id !== "attention" && (i === 0 || all[i - 1].id === "attention") ? <Fragment key="in-progress">{progress}</Fragment> : null,
            <section key={g.id} className="tx-group card-l1" data-testid={`mn-group-${g.id}`}>
              <h2 className="contacts-group-title">
                {g.id === "retired" ? (
                  <button type="button" className="contacts-disclosure" aria-expanded={retiredOpen} data-testid="mn-retired-toggle" onClick={() => setRetiredOpen((v) => !v)}>
                    {t("masternodes.myMasternodes.retiredCount", { count: g.rows.length })}
                  </button>
                ) : (
                  GROUP_TITLE[g.id]
                )}
              </h2>
              {(g.id !== "retired" || retiredOpen) && (
                <ul className="mg-list">
                  {g.rows.map((r) => (
                    <li key={r.proTxHash}>
                      <button
                        type="button"
                        className="mg-row mg-row-wide"
                        aria-current={r.proTxHash === current?.proTxHash ? "true" : undefined}
                        data-testid={`mn-row-${r.proTxHash}`}
                        onClick={() => setSelected(r.proTxHash)}
                      >
                        <span className="mg-row-text">
                          <span className="mg-row-title">{rowName(r)}</span>
                          <span className="mg-row-sub">{rowSub(r)}</span>
                        </span>
                        <span className="mg-row-trailing">
                          <StatusChip chip={statusChip(r.status)} />
                          {rowMeta(r, synced) && <span className="mg-row-meta">{rowMeta(r, synced)}</span>}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>,
          ])}
          {all.every((g) => g.id === "attention") && progress}
        </div>
        {current && <Pane key={current.proTxHash} row={current} rows={o.masternodes} wallet={wallet} network={network} shown={shown} onVote={onVote} />}
      </div>
    </>
  );
}

function Pane({ row, rows, wallet, network, shown, onVote }: { row: MasternodeRow; rows: MasternodeRow[]; wallet: WalletSummary; network: NetworkInfo; shown: Display; onVote: () => void }) {
  const t = useT();
  const detail = useMasternode(wallet.walletId, row.proTxHash);
  const backend = useBackend();
  const [since] = useState(() => Math.floor(backend.now().getTime() / 1000) - 30 * DAY);
  const payouts = useMasternodePayouts(wallet.walletId, row.proTxHash, since, row.mine && !row.sharedUnsupported);
  const refresh = useMasternodesRefresh();
  const [renaming, setRenaming] = useState(false);
  const [releasing, setReleasing] = useState(false);
  const [lockError, setLockError] = useState<string | null>(null);
  const sends = registrationSends(network);
  const holdsCollateral = row.mine && row.roles.collateral && row.status !== "removed" && !wallet.watchOnly;
  const lockAgain = async () => {
    setLockError(null);
    try {
      await registrationCalls(backend, sends).mnLockCollateral(wallet.walletId, row.proTxHash);
    } catch (e) {
      setLockError(collateralErrorText(e, t));
    }
    void refresh();
  };
  const [sheet, setSheet] = useState<"change-keys" | "operator" | null>(null);
  const d = detail.isError ? undefined : detail.data;
  const name = rowName(row);
  const blocked = voteBlocked(row, wallet.watchOnly);
  const outpoint = row.collateral.kind === "known" ? row.collateral.outpoint : null;
  const line = d ? registrarLine(d) : null;
  const canChange = d?.registrar.kind === "available" && !wallet.watchOnly && row.status !== "removed";
  const mayChange = detail.isPending && !wallet.watchOnly && row.status !== "removed";
  return (
    <aside className="contacts-pane" aria-label={t("masternodes.myMasternodes.pane")} aria-busy={detail.isLoading || payouts.isLoading || undefined} data-testid="mn-pane">
      <section className="mg-pane card-l1">
        <div>
          <h2 className="mg-pane-title" data-testid="mn-pane-title">
            {name}
          </h2>
          <div className="chip-row mg-pane-chip">
            <StatusChip chip={statusChip(row.status)} testId="mn-pane-chip" />
            <span className="chip chip-neutral">{kindText(row.kind)}</span>
          </div>
        </div>
        <div className="mg-pane-actions">
          <div className="mg-actions">
            {row.mine && (
              <DashButton variant="tintedBlue" size="small" data-testid="mn-vote" disabled={blocked !== null} why={blocked} onClick={onVote}>
                {MnText.vote}
              </DashButton>
            )}
            {canChange ? (
              <DashButton variant="tintedBlue" size="small" data-testid="mn-change-keys" onClick={() => setSheet("change-keys")}>
                {t("masternodes.myMasternodes.changeKeys.action")}
              </DashButton>
            ) : (
              mayChange && (
                <DashButton variant="tintedBlue" size="small" className="mg-reserved" aria-hidden tabIndex={-1} disabled>
                  {t("masternodes.myMasternodes.changeKeys.action")}
                </DashButton>
              )
            )}
          </div>
          <MoreMenu
            testId="mn-more"
            items={[
              { id: "rename", title: MnText.rename, onSelect: () => setRenaming(true) },
              { id: "copy-hash", title: MnText.copyHash, onSelect: () => void copyWithToast(row.proTxHash) },
              ...(outpoint ? [{ id: "copy-outpoint", title: MnText.copyOutpoint, onSelect: () => void copyWithToast(outpoint) }] : []),
              ...(holdsCollateral && sends
                ? [
                    row.collateralReleased
                      ? { id: "lock-again", title: t("masternodes.release.lockAgain"), onSelect: () => void lockAgain() }
                      : { id: "release", title: t("masternodes.release.action"), onSelect: () => setReleasing(true) },
                  ]
                : []),
              ...(row.status !== "removed" ? [{ id: "operator", title: t("masternodes.myMasternodes.operatorSheet.menu"), onSelect: () => setSheet("operator") }] : []),
            ]}
          />
        </div>
        {row.status === "pose_banned" && (
          <p className="mg-notice mg-notice-danger mn-banned-notice" data-testid="mn-banned">
            <strong>{MnText.banned}</strong> {MnText.bannedText(row.bannedSeenAt === null ? null : shortDate(row.bannedSeenAt))}
            {canChange && <> {t("masternodes.myMasternodes.bannedMove")}</>}
          </p>
        )}
        {detail.isError && <p className="mg-field-error" data-testid="mn-detail-failed">{mnErrorText(detail.error)}</p>}
        {holdsCollateral && row.collateralReleased && (
          <p className="mg-notice mg-notice-warning mg-notice-row" data-testid="mn-released">
            <span>{t("masternodes.release.released")}</span>
            {sends && (
              <DashButton variant="tintedBlue" size="small" data-testid="mn-lock-again" onClick={() => void lockAgain()}>
                {t("masternodes.release.lockAgain")}
              </DashButton>
            )}
          </p>
        )}
        {lockError && (
          <p className="mg-field-error" role="alert" data-testid="mn-lock-error">
            {lockError}
          </p>
        )}
        <dl className="mg-facts" data-testid="mn-facts">
          <Facts row={row} detail={d} shown={shown} />
        </dl>
        <Payouts payouts={payouts} row={row} rows={rows} name={name} shown={shown} />
        <div>
          <h3 className="mg-section-title">{t("masternodes.myMasternodes.inWallet")}</h3>
          <ul className="mg-roles" data-testid="mn-roles">
            {heldRoles(row, wallet.watchOnly).map((h) => (
              <li key={h.id} className={h.held ? "mg-held" : "mg-elsewhere"}>
                {h.text}
              </li>
            ))}
          </ul>
        </div>
        {line && (
          <p className="mg-small mg-muted mg-notice-row" data-testid="mn-registrar-line">
            <span>{line.text}</span>
            {line.unlock && (
              <DashButton variant="tintedBlue" size="small" data-testid="mn-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
                {MnText.unlock}
              </DashButton>
            )}
          </p>
        )}
      </section>
      {renaming && <RenameSheet row={row} onClose={() => setRenaming(false)} />}
      {releasing && <ReleaseSheet row={row} wallet={wallet} network={network} onClose={() => setReleasing(false)} />}
      {sheet === "change-keys" && d && canChange && <ChangeKeysSheet wallet={wallet} network={network} row={row} detail={d} onClose={() => setSheet(null)} />}
      {sheet === "operator" && <OperatorSheet row={row} detail={d} onChangeKeys={canChange ? () => setSheet("change-keys") : null} onClose={() => setSheet(null)} />}
    </aside>
  );
}

function Facts({ row, detail, shown }: { row: MasternodeRow; detail: MasternodeDetail | undefined; shown: Display }) {
  const t = useT();
  const list = detail?.list ?? null;
  const reg = detail?.registration ?? null;
  const fact = (id: string, title: string, value: React.ReactNode) => [
    <dt key={`${id}-t`}>{title}</dt>,
    <dd key={`${id}-d`} data-testid={`mn-fact-${id}`}>
      {value}
    </dd>,
  ];
  const last = row.lastPayout;
  return (
    <>
      {row.service && fact("service", t("masternodes.myMasternodes.fact.service"), row.service)}
      {reg?.registeredHeight != null && fact("registered", t("masternodes.myMasternodes.fact.registered"), t("masternodes.myMasternodes.fact.registeredValue", { height: reg.registeredHeight }))}
      {row.mine && reg?.payoutAddress && fact("payout", t("masternodes.myMasternodes.fact.payout"), <>
        <span className="mono">{shortAddress(reg.payoutAddress)}</span> <span className="mg-muted">{MnText.payoutProvenance}</span>
      </>)}
      {reg?.operatorReward ? fact("operator-share", t("masternodes.myMasternodes.fact.operatorShare"), operatorShare(reg.operatorReward)) : null}
      {list && fact("voting", t("masternodes.myMasternodes.fact.voting"), <span className="mono">{shortAddress(list.votingAddress)}</span>)}
      {list?.platformNodeId &&
        fact(
          "platform",
          t("masternodes.myMasternodes.fact.platform"),
          <span className="mono">
            {list.platformHttpPort
              ? t("masternodes.myMasternodes.fact.platformValuePort", { node: shortHash(list.platformNodeId), port: String(list.platformHttpPort) })
              : t("masternodes.myMasternodes.fact.platformValue", { node: shortHash(list.platformNodeId) })}
          </span>,
        )}
      {last &&
        fact(
          "last-payout",
          lastPayoutTitle(last),
          last.unattributed ? (
            last.timestamp !== null && shortDate(last.timestamp)
          ) : last.timestamp !== null ? (
            t.rich("masternodes.myMasternodes.fact.lastPayoutValue", { amount: <AmountText text={shown.compact(last.amount)} unitName={shown.name} />, date: shortDate(last.timestamp) })
          ) : (
            <AmountText text={shown.compact(last.amount)} unitName={shown.name} />
          ),
        )}
      {fact("pose", t("masternodes.myMasternodes.fact.pose"), <>
        — <span className="mg-muted">{MnText.poseScore}</span>
      </>)}
      {reg?.revoked && fact("revoked", t("masternodes.myMasternodes.fact.revoked"), t("masternodes.myMasternodes.fact.yes"))}
      {fact("hash", t("masternodes.myMasternodes.fact.hash"), (
        <span className="copy-value">
          <span className="mono">{`${row.proTxHash.slice(0, 4)}…${row.proTxHash.slice(-4)}`}</span>
          <CopyIconButton text={row.proTxHash} label={MnText.copyHash} />
        </span>
      ))}
    </>
  );
}

function Payouts({ payouts, row, rows, name, shown }: { payouts: ReturnType<typeof useMasternodePayouts>; row: MasternodeRow; rows: MasternodeRow[]; name: string; shown: Display }) {
  const t = useT();
  if (payouts.isError)
    return (
      <p className="mg-small mg-notice-row" data-testid="mn-payouts-failed">
        <span className="mg-field-error">{t("masternodes.myMasternodes.payoutsFailedWith", { reason: mnErrorText(payouts.error) })}</span>
        <DashButton variant="tintedBlue" size="small" data-testid="mn-payouts-retry" onClick={() => void payouts.refetch()}>
          {MnText.tryAgain}
        </DashButton>
      </p>
    );
  const list = payoutGroups(payouts.data ?? []);
  if (list.length === 0) return null;
  return (
    <div data-testid="mn-payouts">
      {list.map((g) => (
        <div key={g.heading} data-testid={g.flagged ? "mn-payouts-flagged" : "mn-payouts-attributed"}>
          <h3 className="mg-section-title">{g.heading}</h3>
          <p className="mg-small">
            {t.rich("masternodes.myMasternodes.payoutsIn", { amount: <AmountText text={shown.compact(g.total)} unitName={shown.name} />, count: g.count })} ·{" "}
            <DashButton
              variant="plainBlue"
              size="small"
              data-testid={g.flagged ? "mn-flagged-activity" : "mn-payouts-activity"}
              onClick={() => navigate("activity", { activityFilter: { kind: "txids", title: activityChip(name, g.flagged), txids: g.txids } })}
            >
              {MnText.seeInActivity}
            </DashButton>
          </p>
        </div>
      ))}
      {sharesPayoutAddress(row, rows) && <p className="mg-small mg-muted" data-testid="mn-shared-payout">{MnText.sharedPayout}</p>}
    </div>
  );
}

function RenameSheet({ row, onClose }: { row: MasternodeRow; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const refresh = useMasternodesRefresh();
  const [label, setLabel] = useState(row.label ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setBusy(true);
    try {
      await backend.mnSetLabel(row.proTxHash, label.trim() || null);
      await refresh();
      onClose();
    } catch (e) {
      setError(mnErrorText(e));
      setBusy(false);
    }
  };
  return (
    <Sheet title={MnText.renameTitle} width="small" testId="mn-rename-sheet" onClose={onClose}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <label className={`mg-field ${error ? "has-error" : ""}`}>
          {t("masternodes.myMasternodes.renameName")}
          <input data-testid="mn-rename-input" value={label} maxLength={MAX_LABEL} placeholder={rowName({ ...row, label: null })} autoComplete="off" data-autofocus onChange={(e) => setLabel(e.target.value)} />
          <span className="mg-field-help">{t("masternodes.myMasternodes.renameHelp")}</span>
          {error && (
            <span className="mg-field-error" role="alert" data-testid="mn-rename-error">
              {error}
            </span>
          )}
        </label>
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            <DashButton type="button" variant="tintedGray" size="medium" onClick={onClose}>
              {t("masternodes.myMasternodes.cancel")}
            </DashButton>
            <DashButton type="submit" variant="filledBlue" size="medium" data-testid="mn-rename-save" disabled={busy}>
              {t("masternodes.myMasternodes.save")}
            </DashButton>
          </span>
        </footer>
      </form>
    </Sheet>
  );
}

