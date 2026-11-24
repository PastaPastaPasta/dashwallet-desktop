// My masternodes ▸ In progress (MG-10 §1.10): every ProTx operation not yet settled, with the
// action its state allows. Send again asks a `masternodeOp` grant capped at the row's `debit`
// (EP 2); Discard releases the coins of a row nothing of which may be out. A conflicted row shows
// once as a notice, with no action. Shared sessions join this group in S5.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { lockGeneration, refusedWhileLocked, walletGateClosed } from "../../api/lockGate";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { navigate } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { mnErrorText } from "../../masternodes/model";
import { actsOn, conflicted, inProgress, opRow, opTitle, proTxSends, resendCap, resendConfirmText, type OpAction } from "../../masternodes/operations";
import { useMasternodesRefresh } from "../../masternodes/queries";
import { MoreMenu, StatusChip } from "./common";
import { CredentialField, useCredential } from "./credential";

const I = "masternodes.inProgress";

/** The group's open confirmation and a row's refusal. Held by the page, not the group, so a list
 * read that changes the page's layout neither closes an open sheet nor drops its refusal. */
export interface InProgressActions {
  act: (o: ProTxOperation, a: OpAction) => void;
  error: { id: string; text: string } | null;
  /** Send again or Discard, when open: rendered by the page outside the list's layouts. */
  sheets: ReactNode;
}

export function useInProgressActions(wallet: WalletSummary, network: NetworkInfo, onContinue: (op: ProTxOperation) => void): InProgressActions {
  const refresh = useMasternodesRefresh();
  const b = useBackend();
  const [resending, setResending] = useState<ProTxOperation | null>(null);
  const [discarding, setDiscarding] = useState<ProTxOperation | null>(null);
  const [error, setError] = useState<{ id: string; text: string } | null>(null);
  const act = (o: ProTxOperation, a: OpAction) => {
    setError(null);
    if (a === "continue" || a === "refresh") onContinue(o);
    else if (a === "resend") setResending(o);
    else setDiscarding(o);
  };
  const sheets = (
    <>
      {resending && <ResendSheet op={resending} wallet={wallet} network={network} onClose={() => setResending(null)} returnFocus={rowAction(resending.id, "resend")} />}
      {discarding && (
        <DiscardSheet
          op={discarding}
          onClose={() => setDiscarding(null)}
          returnFocus={rowAction(discarding.id, "discard")}
          onConfirm={async () => {
            const o = discarding;
            try {
              await b.mnDiscardOperation(wallet.walletId, o.id);
              setDiscarding(null);
              void refresh();
            } catch (e) {
              setDiscarding(null);
              setError({ id: o.id, text: mnErrorText(e) });
              void refresh();
            }
          }}
        />
      )}
    </>
  );
  return { act, error, sheets };
}

export function InProgressGroup({
  ops,
  wallet,
  network,
  actions,
}: {
  ops: readonly ProTxOperation[];
  wallet: WalletSummary;
  network: NetworkInfo;
  actions: InProgressActions;
}) {
  const t = useT();
  const { act, error } = actions;
  const sends = proTxSends(network);
  const rows = inProgress(ops);
  const replaced = conflicted(ops);
  if (rows.length === 0 && replaced.length === 0) return null;

  const label = (a: OpAction) => t(`${I}.${a === "continue" ? "continue" : a === "refresh" ? "newMessage" : a === "resend" ? "sendAgain" : "discard"}`);

  return (
    <section className="tx-group card-l1" data-testid="mn-group-in-progress">
      <h2 className="contacts-group-title">{t(`${I}.group`)}</h2>
      {replaced.map((o) => (
        <p key={o.id} className="mg-notice mg-notice-warning mg-small" data-testid={`mn-op-conflicted-${o.id}`}>
          <strong>{opTitle(o.kind)}</strong> {t(`${I}.conflicted`)}
        </p>
      ))}
      <ul className="mg-list">
        {rows.map((o) => {
          const v = opRow(o, sends);
          return (
            <li key={o.id} className="mg-row mg-row-wide mg-op" data-testid={`mn-op-${o.id}`} data-state={o.state.kind}>
              <span className="mg-row-text">
                <span className="mg-row-title">{v.title}</span>
                <span className="mg-row-sub" data-testid={`mn-op-status-${o.id}`}>
                  {v.status}
                </span>
                {v.overdueLine && (
                  <span className="mg-small mg-muted" data-testid={`mn-op-overdue-${o.id}`}>
                    {v.overdueLine}
                  </span>
                )}
                {error?.id === o.id && (
                  <span className="mg-field-error" role="alert" data-testid={`mn-op-error-${o.id}`}>
                    {error.text}
                  </span>
                )}
              </span>
              <span className="mg-row-trailing">
                {o.state.kind === "needs_signature" && <StatusChip chip={{ text: t(`${I}.chip`), tone: "warning", tooltip: "" }} />}
                {v.activityTxid && (
                  <DashButton
                    variant="plainBlue"
                    size="small"
                    data-testid={`mn-op-activity-${o.id}`}
                    onClick={() => navigate("activity", { activityFilter: { kind: "txids", title: t(`${I}.activityChip`, { title: v.title }), txids: [v.activityTxid!] } })}
                  >
                    {t(`${I}.openInActivity`)}
                  </DashButton>
                )}
                {v.primary && !wallet.watchOnly && (
                  <DashButton variant="tintedBlue" size="small" data-testid={`mn-op-${v.primary}-${o.id}`} onClick={() => act(o, v.primary!)}>
                    {label(v.primary)}
                  </DashButton>
                )}
                {v.more.length > 0 && !wallet.watchOnly && (
                  <MoreMenu testId={`mn-op-more-${o.id}`} items={v.more.map((a) => ({ id: a, title: label(a), onSelect: () => act(o, a) }))} />
                )}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

type ReturnFocus = () => HTMLElement | null;

// A list read can replace the row, so the opener may be gone when the sheet closes.
const rowAction = (id: string, action: OpAction): ReturnFocus => () =>
  document.querySelector<HTMLElement>(`[data-testid="mn-op-${action}-${id}"]`) ?? document.querySelector<HTMLElement>(`[data-testid="mn-op-more-${id}"]`);

function DiscardSheet({
  op,
  onClose,
  onConfirm,
  returnFocus,
}: {
  op: ProTxOperation;
  onClose: () => void;
  onConfirm: () => Promise<void>;
  returnFocus: ReturnFocus;
}) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  return (
    <Sheet title={t(`${I}.discardTitle`)} width="small" testId="mn-op-discard-sheet" closable={!busy} onClose={onClose} returnFocus={returnFocus}>
      <div className="mg-sheet-body">
        <p>{t(`${I}.discardText.${op.kind}`)}</p>
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" onClick={onClose} disabled={busy}>
              {t(`${I}.cancel`)}
            </DashButton>
            <DashButton
              variant="plainRed"
              size="medium"
              data-testid="mn-op-discard-confirm"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void onConfirm().finally(() => setBusy(false));
              }}
            >
              {t(`${I}.discard`)}
            </DashButton>
          </span>
        </footer>
      </div>
    </Sheet>
  );
}

function ResendSheet({
  op,
  wallet,
  network,
  onClose,
  returnFocus,
}: {
  op: ProTxOperation;
  wallet: WalletSummary;
  network: NetworkInfo;
  onClose: () => void;
  returnFocus: ReturnFocus;
}) {
  const t = useT();
  const b = useBackend();
  const shown = useDisplay(network);
  const refresh = useMasternodesRefresh();
  const credential = useCredential(wallet.walletId, t(`${I}.ask`), resendCap(op));
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const send = async () => {
    if (busy || !credential.ready || !actsOn(op, proTxSends(network))) return;
    const gen = lockGeneration();
    setBusy(true);
    let granted: string | null = null;
    try {
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: resendCap(op) });
      if (!grant) return;
      granted = grant.id;
      if (!mounted.current || walletGateClosed() || lockGeneration() !== gen) {
        void b.revokeGrant(grant.id).catch(() => null);
        return;
      }
      await b.mnResendOperation(wallet.walletId, op.id, grant.id);
      void refresh();
      if (mounted.current) onClose();
    } catch (e) {
      if (granted) void b.revokeGrant(granted).catch(() => null);
      if (refusedWhileLocked(e) || !mounted.current) return;
      credential.setError(mnErrorText(e));
      void refresh();
    } finally {
      if (mounted.current) setBusy(false);
    }
  };
  return (
    <Sheet title={t(`${I}.resendTitle`)} width="small" testId="mn-op-resend-sheet" closable={!busy} onClose={onClose} returnFocus={returnFocus}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <p data-testid="mn-op-resend-text">{resendConfirmText(op, shown.withUnit(resendCap(op)))}</p>
        <CredentialField c={credential} busy={busy} testId="mn-op-resend" />
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" onClick={onClose} disabled={busy}>
              {t(`${I}.cancel`)}
            </DashButton>
            <DashButton type="submit" variant="filledBlue" size="medium" data-testid="mn-op-resend-confirm" disabled={busy || !credential.ready}>
              {t(`${I}.sendAgain`)}
            </DashButton>
          </span>
        </footer>
      </form>
    </Sheet>
  );
}
