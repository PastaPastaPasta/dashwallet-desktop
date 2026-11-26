// A shared masternode's maintenance (MG-10 §2.6–§2.8, S6): Dissolve (`sh-dissolve`: alone, together,
// standby), the standby's create, save and send sheets, and Change keys together. Each call that
// signs or spends takes a `masternodeOp` grant capped at the engine's own figure (§3.4). An action
// whose amount is unknown, or a send this network refuses (MG's mainnet ruling), is absent (D14).

import { useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { KeyChangeRequest } from "../../api/generated/KeyChangeRequest";
import type { SharedSession } from "../../api/generated/SharedSession";
import type { SharedShares } from "../../api/generated/SharedShares";
import type { Standby } from "../../api/generated/Standby";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { showToast } from "../../components/toast";
import { shortDate } from "../../governance/model";
import { useT } from "../../i18n";
import { mnKeys, useMasternodesRefresh } from "../../masternodes/queries";
import {
  DISSOLVE_FEE,
  STANDBY_FEE,
  dissolveCap,
  labelOf,
  operatorKeyProblem,
  sharedErrorText,
  sharedSendsOn,
  standbyCreateCap,
  standbyLines,
  standbySend,
  standbySource,
  togetherFee,
  type LegendEntry,
  type StandbySend,
} from "../../masternodes/shared";
import { useAmountField } from "../pay/form";
import { CredentialField, useCredential } from "./credential";
import { Notice } from "./common";
import { useSharedText, type OpenSession } from "./Shared";

const P = "masternodes.shared";
const D = `${P}.detail.standby`;

/** What every maintenance sheet reads about the masternode. */
export interface MaintenanceCtx {
  wallet: WalletSummary;
  network: NetworkInfo;
  hash: string;
  shares: SharedShares;
  legend: LegendEntry[];
  /** The registration session, when the wallet holds it. */
  session: SharedSession | null;
  standbys: Standby[];
  onOpenSession: (s: OpenSession) => void;
}

export type MaintenanceSheet =
  | { kind: "dissolve"; segment: Segment }
  | { kind: "create" }
  | { kind: "export"; standby: Standby }
  | { kind: "send"; standby: Standby; send: StandbySend }
  | { kind: "keyChange" };

type Segment = "alone" | "together" | "standby";

const mineOf = (c: MaintenanceCtx) => c.shares.shares.filter((f) => f.mine);

/** The pane's (and the Standby segment's) stored standbys, with Save a copy… and the one send the
 * engine would take now (§2.8). On mainnet no send shows. */
export function Standbys({ c, actions, onSheet }: { c: MaintenanceCtx; actions: boolean; onSheet: (s: MaintenanceSheet) => void }) {
  const t = useT();
  const { dash } = useSharedText(c.network);
  const own = new Set(c.legend.filter((l) => l.mine).map((l) => l.index));
  const labelled = c.standbys.some((s) => !own.has(s.shareIndex));
  const sends = sharedSendsOn(c.network);
  const missing = mineOf(c).some((f) => !c.standbys.some((s) => s.shareIndex === f.index && (s.full || s.immediate)));
  if (c.standbys.length === 0 && !(actions && missing)) return null;
  return (
    <div data-testid="sh-detail-standby">
      <h3 className="mg-section-title">{t(`${D}.title`)}</h3>
      {c.standbys.map((s) => {
        const offer = actions ? standbySend(s, c.shares.fullValidNow, own.has(s.shareIndex)) : null;
        const send = sends ? offer : null;
        const id = `${s.shareIndex}-${s.fee}`;
        return (
          <div key={id} className="sh-standby">
            <ul className="sh-lines mg-small" data-testid={`sh-standby-${id}`}>
              {labelled && <li className="sh-standby-share">{labelOf(c.legend, s.shareIndex)}</li>}
              {standbyLines(s, c.shares.fullValidNow, dash).map((l) => (
                <li key={l.variant} data-testid={`sh-standby-${l.variant}`}>
                  {l.text}
                </li>
              ))}
              <li className="mg-muted">{standbySource(s, (secs) => shortDate(secs))}</li>
              {offer?.stillPenalty && (
                <li className="mg-field-error" data-testid={`sh-standby-still-penalty-${id}`}>
                  {t(`${D}.onlyImmediate`, { penalty: dash(offer.penalty) })}
                </li>
              )}
            </ul>
            {actions && (
              <span className="mg-actions">
                <DashButton variant="tintedGray" size="small" data-testid={`sh-standby-save-${id}`} onClick={() => onSheet({ kind: "export", standby: s })}>
                  {t(`${D}.saveCopy`)}
                </DashButton>
                {offer?.stillPenalty && own.has(s.shareIndex) && (
                  <DashButton variant="tintedBlue" size="small" data-testid={`sh-standby-create-full-${id}`} onClick={() => onSheet({ kind: "create" })}>
                    {t(`${D}.createFull`)}
                  </DashButton>
                )}
                {send && (
                  <DashButton variant="tintedBlue" size="small" data-testid={`sh-standby-send-${id}`} onClick={() => onSheet({ kind: "send", standby: s, send })}>
                    {t(`${D}.${send.label}`)}
                  </DashButton>
                )}
              </span>
            )}
          </div>
        );
      })}
      {actions && missing && (
        <Notice
          tone="info"
          testId="sh-standby-prompt"
          action={
            <DashButton variant="filledBlue" size="small" data-testid="sh-standby-create" onClick={() => onSheet({ kind: "create" })}>
              {t(`${D}.createIt`)}
            </DashButton>
          }
        >
          {t(`${D}.prompt`)}
        </Notice>
      )}
    </div>
  );
}

/** The sheet the pane has open, if any. */
export function MaintenanceSheets({ c, sheet, onSheet }: { c: MaintenanceCtx; sheet: MaintenanceSheet | null; onSheet: (s: MaintenanceSheet | null) => void }) {
  if (!sheet) return null;
  const close = () => onSheet(null);
  switch (sheet.kind) {
    case "dissolve":
      return <DissolveSheet c={c} segment={sheet.segment} onSheet={onSheet} onClose={close} />;
    case "create":
      return <CreateStandbySheet c={c} onClose={close} />;
    case "export":
      return <ExportStandbySheet c={c} standby={sheet.standby} onClose={close} />;
    case "send":
      return <SendStandbySheet c={c} standby={sheet.standby} send={sheet.send} onClose={close} />;
    case "keyChange":
      return <KeyChangeSheet c={c} onClose={close} />;
  }
}

/** A call under a grant: busy, the refusal as a sentence, and a re-read afterwards. */
function useGranted(c: MaintenanceCtx, ask: string) {
  const refresh = useMasternodesRefresh();
  const { dash, networkName } = useSharedText(c.network);
  const credential = useCredential(c.wallet.walletId, ask, null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async <T,>(cap: number, call: (grant: string) => Promise<T>): Promise<T | null> => {
    if (busy || !credential.ready) return null;
    setBusy(true);
    setError(null);
    try {
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: cap });
      if (!grant) return null;
      return await call(grant.id);
    } catch (e) {
      setError(sharedErrorText(e, { network: networkName, session: c.session, dash }));
      return null;
    } finally {
      setBusy(false);
      void refresh();
    }
  };
  return { credential, busy, error, run };
}

function Problem({ text, testId }: { text: string | null; testId: string }) {
  return (
    <p className="form-problem" role="alert" data-testid={testId}>
      {text}
    </p>
  );
}

function DoneFooter({ onClose, testId }: { onClose: () => void; testId: string }) {
  const t = useT();
  return (
    <footer className="mg-sheet-footer">
      <span />
      <DashButton variant="filledBlue" size="medium" data-testid={testId} data-autofocus onClick={onClose}>
        {t(`${P}.dissolve.done`)}
      </DashButton>
    </footer>
  );
}

/** `sh-dissolve` (§2.7): Alone, now · Together · Standby. */
function DissolveSheet({ c, segment: first, onSheet, onClose }: { c: MaintenanceCtx; segment: Segment; onSheet: (s: MaintenanceSheet) => void; onClose: () => void }) {
  const t = useT();
  const [segment, setSegment] = useState<Segment>(first);
  const segments: Segment[] = ["alone", "together", "standby"];
  return (
    <Sheet title={t(`${P}.dissolve.title`)} width="medium" testId="sh-dissolve" onClose={onClose}>
      <div className="mg-sheet-body mg-step">
        <div className="segmented" role="group" aria-label={t(`${P}.dissolve.title`)}>
          {segments.map((s) => (
            <button key={s} type="button" className="segment" data-testid={`sh-dissolve-${s}`} aria-pressed={segment === s} onClick={() => setSegment(s)}>
              {t(`${P}.dissolve.segments.${s}`)}
            </button>
          ))}
        </div>
        {segment === "alone" && <Alone c={c} onClose={onClose} />}
        {segment === "together" && <Together c={c} onClose={onClose} />}
        {segment === "standby" && (
          <>
            {c.standbys.length === 0 && !mineOf(c).length && <p className="mg-muted">{t(`${P}.dissolve.noStandby`)}</p>}
            <Standbys c={c} actions onSheet={onSheet} />
            {!sharedSendsOn(c.network) && <MainnetDissolveNote />}
          </>
        )}
      </div>
    </Sheet>
  );
}

export function MainnetDissolveNote() {
  const t = useT();
  return (
    <Notice tone="info" testId="sh-mainnet-dissolve-note">
      {t(`${P}.mainnetDissolveNote`)}
    </Notice>
  );
}

/** Alone, now: the preview's figures, the early-exit consent, then the passphrase (§2.7). */
function Alone({ c, onClose }: { c: MaintenanceCtx; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const { dash, networkName } = useSharedText(c.network);
  const mine = mineOf(c);
  const [share, setShare] = useState(mine[0]?.index ?? 0);
  const [accepted, setAccepted] = useState(false);
  const [sent, setSent] = useState(false);
  const pickId = useId();
  const sends = sharedSendsOn(c.network);
  const g = useGranted(c, t(`${P}.dissolve.ask`));
  const preview = useQuery({
    queryKey: [...mnKeys.detail(c.wallet.walletId, c.hash), "previewDissolve", share, DISSOLVE_FEE],
    queryFn: () => b.mnSharedPreviewDissolve(c.wallet.walletId, { proTxHash: c.hash, shareIndex: share, fee: DISSOLVE_FEE }),
    enabled: c.session !== null && sends && !sent,
    retry: false,
  });
  if (!c.session) {
    const text = c.shares.fullValidNow ? t(`${P}.dissolve.noSessionFull`) : t(`${P}.dissolve.noSession`, { penalty: dash(c.shares.earlyPenalty) });
    return (
      <p className="mg-muted" data-testid="sh-dissolve-no-session">
        {text}
      </p>
    );
  }
  if (!sends) return <MainnetDissolveNote />;
  if (sent)
    return (
      <div data-testid="sh-dissolve-sent">
        <p>{t(`${P}.dissolve.sent`)}</p>
        <DoneFooter onClose={onClose} testId="sh-dissolve-done" />
      </div>
    );
  const p = preview.data;
  const dust = p?.warnings.some((w) => w.kind === "actor_output_dust") ?? false;
  const back = p?.returns.filter((r) => r.mine && r.shareIndex === share).reduce((a, r) => a + r.amount, 0) ?? 0;
  const ready = p !== undefined && !dust && (!p.early || accepted) && g.credential.ready && !g.busy && !c.wallet.watchOnly;
  const dissolve = async () => {
    if (!p || !ready) return;
    const op = await g.run(dissolveCap(p), (grant) => b.mnSharedDissolve(c.wallet.walletId, { proTxHash: c.hash, shareIndex: share, fee: p.fee }, grant));
    if (op) setSent(true);
  };
  return (
    <form
      className="mg-step"
      data-testid="sh-dissolve-alone"
      onSubmit={(e) => {
        e.preventDefault();
        void dissolve();
      }}
    >
      <p className="mg-muted">{t(`${P}.dissolve.aloneIntro`)}</p>
      {mine.length > 1 && (
        <div className="mg-choices" role="radiogroup" aria-label={t(`${P}.dissolve.pickShare`)}>
          <p className="mg-muted mg-small">{t(`${P}.dissolve.pickShare`)}</p>
          {mine.map((f) => (
            <label key={f.index} className="mg-choice">
              <input type="radio" name={`${pickId}-share`} checked={share === f.index} onChange={() => (setShare(f.index), setAccepted(false))} />
              <span className="mg-choice-text">{labelOf(c.legend, f.index)}</span>
            </label>
          ))}
        </div>
      )}
      {preview.isError ? (
        <p className="mg-field-error" role="alert" data-testid="sh-dissolve-failed">
          {t(`${P}.dissolve.failed`, { reason: sharedErrorText(preview.error, { network: networkName, session: c.session, dash }) })}
        </p>
      ) : !p ? (
        <p className="mg-muted" aria-live="polite">
          {t(`${P}.dissolve.checking`)}
        </p>
      ) : (
        <>
          <dl className="mg-facts" data-testid="sh-dissolve-figures">
            <dt>{t(`${P}.dissolve.youGetBack`)}</dt>
            <dd data-testid="sh-dissolve-back">{dash(back)}</dd>
            <dt>{t(`${P}.dissolve.others`)}</dt>
            <dd>
              <ul className="sh-lines mg-small" data-testid="sh-dissolve-others">
                {p.returns
                  .filter((r) => r.shareIndex !== share)
                  .map((r) => (
                    <li key={r.shareIndex}>{t(`${P}.detail.legend`, { label: labelOf(c.legend, r.shareIndex), amount: dash(r.amount) })}</li>
                  ))}
              </ul>
            </dd>
            <dt>{t(`${P}.dissolve.fee`)}</dt>
            <dd>{t(`${P}.dissolve.feeValue`, { fee: dash(p.fee) })}</dd>
          </dl>
          {p.early && (
            <Notice tone="warning" testId="sh-dissolve-early">
              {t.rich(`${P}.dissolve.early`, { strong: (x) => <strong>{x}</strong>, penalty: dash(p.penalty), height: p.validFromHeight })}
            </Notice>
          )}
          {dust ? (
            <Notice tone="warning" testId="sh-dissolve-dust">
              {t(`${P}.dissolve.dust`)}
            </Notice>
          ) : (
            <>
              <p className="sh-row-copy" data-testid="sh-dissolve-leaves">
                {p.penalty > 0
                  ? t(`${P}.dissolve.leavesPenalty`, { total: dash(dissolveCap(p)), penalty: dash(p.penalty), fee: dash(p.fee) })
                  : t(`${P}.dissolve.leavesFee`, { total: dash(dissolveCap(p)) })}
              </p>
              {p.early && (
                <label className="mg-choice" data-testid="sh-dissolve-accept">
                  <input type="checkbox" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
                  <span className="mg-choice-text">{t(`${P}.dissolve.accept`, { penalty: dash(p.penalty) })}</span>
                </label>
              )}
              <CredentialField c={g.credential} busy={g.busy} testId="sh-dissolve-credential" />
            </>
          )}
        </>
      )}
      <Problem text={g.error} testId="sh-dissolve-error" />
      {p && !dust && (
        <footer className="mg-sheet-footer">
          <span />
          <DashButton type="submit" variant="plainRed" size="medium" data-testid="sh-dissolve-now" disabled={!ready}>
            {t(`${P}.dissolve.button`)}
          </DashButton>
        </footer>
      )}
    </form>
  );
}

/** Together: start a dissolution every owner signs, this wallet's share paying the fee (§2.7). */
function Together({ c, onClose }: { c: MaintenanceCtx; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const { shown, dash } = useSharedText(c.network);
  const n = c.shares.shares.length;
  const least = togetherFee(n);
  const amount = useAmountField(shown, shown.full(least));
  const id = useId();
  const g = useGranted(c, t(`${P}.dissolve.togetherAsk`));
  const below = typeof amount.duffs === "number" && amount.duffs < least;
  const fee = amount.settled && typeof amount.duffs === "number" && !below ? amount.duffs : null;
  const share = mineOf(c)[0]?.index ?? null;
  const ready = fee !== null && share !== null && g.credential.ready && !g.busy && !c.wallet.watchOnly;
  const start = async () => {
    if (!ready) return;
    const out = await g.run(fee, (grant) => b.mnSharedStartDissolveTogether(c.wallet.walletId, { proTxHash: c.hash, shareIndex: share, fee }, grant));
    if (out) {
      onClose();
      c.onOpenSession({ id: out.session.id, imported: null });
    }
  };
  return (
    <form
      className="mg-step"
      data-testid="sh-dissolve-together-form"
      onSubmit={(e) => {
        e.preventDefault();
        void start();
      }}
    >
      <p className="mg-muted">{t(`${P}.dissolve.togetherText`)}</p>
      <div className={`form-field ${below || amount.error ? "has-error" : ""}`}>
        <label htmlFor={id}>{t(`${P}.dissolve.fee`)}</label>
        <span className="mg-field-row">
          <input
            id={id}
            inputMode="decimal"
            placeholder={shown.placeholder}
            data-testid="sh-together-fee"
            aria-invalid={below || !!amount.error}
            value={amount.text}
            onChange={(e) => amount.setText(e.target.value)}
          />
          <span>{shown.name}</span>
        </span>
        {amount.error ? (
          <span className="mg-field-error" role="alert" data-testid="sh-together-fee-error">
            {amount.error}
          </span>
        ) : (
          <span className={below ? "mg-field-error" : "mg-field-help"} role={below ? "alert" : undefined} data-testid="sh-together-fee-help">
            {t(`${P}.dissolve.togetherFeeHelp`, { min: dash(least), count: n })}
          </span>
        )}
      </div>
      <CredentialField c={g.credential} busy={g.busy} testId="sh-together-credential" />
      <Problem text={g.error} testId="sh-together-error" />
      <footer className="mg-sheet-footer">
        <span />
        <DashButton type="submit" variant="filledBlue" size="medium" data-testid="sh-together-start" disabled={!ready}>
          {t(`${P}.dissolve.togetherStart`)}
        </DashButton>
      </footer>
    </form>
  );
}

/** Create it (§2.8 prompt): both variants of each own share, capped at Σ (penalty + fee). */
function CreateStandbySheet({ c, onClose }: { c: MaintenanceCtx; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const g = useGranted(c, t(`${D}.createAsk`));
  const create = async () => {
    const out = await g.run(standbyCreateCap(c.shares, STANDBY_FEE), (grant) => b.mnSharedCreateStandby(c.wallet.walletId, c.hash, STANDBY_FEE, grant));
    if (out) onClose();
  };
  return (
    <Sheet title={t(`${D}.createTitle`)} width="small" testId="sh-standby-create-sheet" closable={!g.busy} onClose={onClose}>
      <form
        className="mg-sheet-body mg-step"
        onSubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <p>{t(`${D}.createText`)}</p>
        <CredentialField c={g.credential} busy={g.busy} testId="sh-standby-create-credential" />
        <Problem text={g.error} testId="sh-standby-create-error" />
        <footer className="mg-sheet-footer">
          <span />
          <DashButton type="submit" variant="filledBlue" size="medium" data-testid="sh-standby-create-confirm" disabled={g.busy || !g.credential.ready || c.wallet.watchOnly}>
            {t(`${D}.createIt`)}
          </DashButton>
        </footer>
      </form>
    </Sheet>
  );
}

/** Save a copy… (§2.8): the alert first, then dw-app's save dialog; only the file's name comes back. */
function ExportStandbySheet({ c, standby, onClose }: { c: MaintenanceCtx; standby: Standby; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const g = useGranted(c, t(`${D}.exportAsk`));
  const [saved, setSaved] = useState<string | null>(null);
  const save = async () => {
    const name = await g.run(0, (grant) => b.mnSharedExportStandby(c.wallet.walletId, c.hash, standby.shareIndex, standby.fee, grant));
    if (name) {
      setSaved(name);
      showToast(t(`${D}.exportSaved`, { name }));
    }
  };
  return (
    <Sheet title={t(`${D}.exportTitle`)} width="small" testId="sh-standby-export" closable={!g.busy} onClose={onClose}>
      <form
        className="mg-sheet-body mg-step"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <Notice tone="warning" testId="sh-export-alert">
          {t(`${D}.exportAlert`)}
        </Notice>
        {saved ? (
          <>
            <p role="status" data-testid="sh-export-saved">
              {t(`${D}.exportSaved`, { name: saved })}
            </p>
            <DoneFooter onClose={onClose} testId="sh-export-done" />
          </>
        ) : (
          <>
            <CredentialField c={g.credential} busy={g.busy} testId="sh-export-credential" />
            <Problem text={g.error} testId="sh-export-error" />
            <footer className="mg-sheet-footer">
              <span />
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="sh-export-save" disabled={g.busy || !g.credential.ready}>
                {t(`${D}.saveCopy`)}
              </DashButton>
            </footer>
          </>
        )}
      </form>
    </Sheet>
  );
}

/** Send a stored standby (§2.8): the variant the engine takes now, capped at fee (+ penalty) for an own
 * share, 0 for another's. */
function SendStandbySheet({ c, standby, send, onClose }: { c: MaintenanceCtx; standby: Standby; send: StandbySend; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const { dash } = useSharedText(c.network);
  const g = useGranted(c, t(`${D}.sendAsk`));
  const [sent, setSent] = useState(false);
  const mine = c.shares.shares.some((f) => f.index === standby.shareIndex && f.mine);
  const go = async () => {
    const op = await g.run(send.cap, (grant) => b.mnSharedSendStandby(c.wallet.walletId, c.hash, standby.shareIndex, standby.fee, send.variant, grant));
    if (op) setSent(true);
  };
  const leaves = !mine
    ? t(`${D}.leavesNothing`)
    : send.variant === "immediate" && send.penalty > 0
      ? t(`${D}.leavesPenalty`, { total: dash(send.cap), penalty: dash(send.penalty), fee: dash(standby.fee) })
      : t(`${D}.leavesFee`, { total: dash(send.cap) });
  return (
    <Sheet title={t(`${D}.sendTitle`)} width="small" testId="sh-standby-send-sheet" closable={!g.busy} onClose={onClose}>
      {sent ? (
        <div className="mg-sheet-body" data-testid="sh-standby-sent">
          <p>{t(`${D}.sent`)}</p>
          <DoneFooter onClose={onClose} testId="sh-standby-sent-done" />
        </div>
      ) : (
        <form
          className="mg-sheet-body mg-step"
          onSubmit={(e) => {
            e.preventDefault();
            void go();
          }}
        >
          {send.stillPenalty ? (
            <Notice tone="warning" testId="sh-standby-send-still-penalty">
              {t(`${D}.onlyImmediate`, { penalty: dash(send.penalty) })}
            </Notice>
          ) : (
            send.variant === "immediate" && (
              <Notice tone="warning" testId="sh-standby-send-penalty">
                {t(`${D}.confirmPenalty`, { penalty: dash(send.penalty) })}
              </Notice>
            )
          )}
          <p className="sh-row-copy" data-testid="sh-standby-send-leaves">
            {leaves}
          </p>
          <CredentialField c={g.credential} busy={g.busy} testId="sh-standby-send-credential" />
          <Problem text={g.error} testId="sh-standby-send-error" />
          <footer className="mg-sheet-footer">
            <span />
            <DashButton type="submit" variant="plainRed" size="medium" data-testid="sh-standby-send-confirm" disabled={g.busy || !g.credential.ready || c.wallet.watchOnly}>
              {t(`${D}.${send.label}`)}
            </DashButton>
          </footer>
        </form>
      )}
    </Sheet>
  );
}

type OperatorPick = "keep" | "keychain" | "given";
type VotingPick = "keep" | "keychain" | "address";

/** Change keys together (§2.6): the operator and voting keys kept or set; every owner signs; this
 * wallet funds it. Starting reserves the fee coins under a grant of 0 and sends nothing. */
function KeyChangeSheet({ c, onClose }: { c: MaintenanceCtx; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const K = "masternodes.myMasternodes.changeKeys";
  const [operator, setOperator] = useState<OperatorPick>("keep");
  const [operatorPub, setOperatorPub] = useState("");
  const [voting, setVoting] = useState<VotingPick>("keep");
  const [address, setAddress] = useState("");
  const g = useGranted(c, t(`${P}.keyChange.ask`));
  const keyProblem = operator === "given" ? operatorKeyProblem(operatorPub) : null;
  const addressMissing = voting === "address" && address.trim() === "";
  const nothing = operator === "keep" && voting === "keep";
  const request: KeyChangeRequest | null =
    nothing || keyProblem || addressMissing
      ? null
      : {
          proTxHash: c.hash,
          operator: operator === "keep" ? { kind: "keep" } : operator === "keychain" ? { kind: "next_unused" } : { kind: "given", publicKey: operatorPub.trim() },
          voting: voting === "keep" ? { kind: "keep" } : voting === "keychain" ? { kind: "next_unused" } : { kind: "address", address: address.trim() },
        };
  const ready = request !== null && g.credential.ready && !g.busy && !c.wallet.watchOnly;
  const start = async () => {
    if (!request || !ready) return;
    const out = await g.run(0, (grant) => b.mnSharedStartKeyChange(c.wallet.walletId, request, grant));
    if (out) {
      onClose();
      c.onOpenSession({ id: out.session.id, imported: null });
    }
  };
  const radio = (group: string, value: string, checked: boolean, pick: () => void, title: string) => (
    <label className="mg-choice" data-testid={`sh-keychange-${group}-${value}`}>
      <input type="radio" name={`sh-keychange-${group}`} value={value} checked={checked} onChange={pick} />
      <span className="mg-choice-text">{title}</span>
    </label>
  );
  return (
    <Sheet title={t(`${P}.keyChange.title`)} width="medium" testId="sh-keychange" closable={!g.busy} onClose={onClose}>
      <form
        className="mg-sheet-body mg-step"
        onSubmit={(e) => {
          e.preventDefault();
          void start();
        }}
      >
        <p className="mg-muted">{t(`${P}.keyChange.intro`)}</p>
        <fieldset className="mg-choices" disabled={g.busy}>
          <legend className="mg-section-title">{t(`${K}.operator.legend`)}</legend>
          {radio("operator", "keep", operator === "keep", () => setOperator("keep"), t(`${K}.operator.keep`))}
          {radio("operator", "keychain", operator === "keychain", () => setOperator("keychain"), t(`${K}.operator.keychain`))}
          {radio("operator", "given", operator === "given", () => setOperator("given"), t(`${K}.operator.given`))}
          {operator === "given" && (
            <label className="mg-field">
              <input className="mono" data-testid="sh-keychange-operator-key" aria-label={t(`${K}.operator.given`)} value={operatorPub} spellCheck={false} autoComplete="off" onChange={(e) => setOperatorPub(e.target.value)} />
              <span className="mg-field-help">{t(`${K}.operator.givenHelp`)}</span>
              {operatorPub.trim() !== "" && keyProblem && <span className="mg-field-error">{t(`${P}.setup.error.operator_${keyProblem}`)}</span>}
            </label>
          )}
        </fieldset>
        <fieldset className="mg-choices" disabled={g.busy}>
          <legend className="mg-section-title">{t(`${K}.voting.legend`)}</legend>
          {radio("voting", "keep", voting === "keep", () => setVoting("keep"), t(`${K}.voting.keep`))}
          {radio("voting", "keychain", voting === "keychain", () => setVoting("keychain"), t(`${K}.voting.keychain`))}
          {radio("voting", "address", voting === "address", () => setVoting("address"), t(`${K}.voting.address`))}
          {voting === "address" && (
            <label className="mg-field">
              <input className="mono" data-testid="sh-keychange-voting-address" aria-label={t(`${K}.voting.address`)} value={address} spellCheck={false} autoComplete="off" onChange={(e) => setAddress(e.target.value)} />
              <span className="mg-field-help">{t(`${K}.voting.addressHelp`)}</span>
            </label>
          )}
        </fieldset>
        {operator !== "keep" && (
          <p className="mg-notice mg-notice-danger" data-testid="sh-keychange-pose">
            <strong>{t(`${K}.poseWarning`)}</strong> {t(`${K}.poseExplain`)}
          </p>
        )}
        {nothing && <p className="mg-muted mg-small">{t(`${P}.keyChange.nothing`)}</p>}
        <CredentialField c={g.credential} busy={g.busy} testId="sh-keychange-credential" />
        <Problem text={g.error} testId="sh-keychange-error" />
        <footer className="mg-sheet-footer">
          <span />
          <DashButton type="submit" variant="filledBlue" size="medium" data-testid="sh-keychange-start" disabled={!ready}>
            {t(`${P}.keyChange.start`)}
          </DashButton>
        </footer>
      </form>
    </Sheet>
  );
}
