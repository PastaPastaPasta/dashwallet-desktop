// Shared masternodes (MG-10 §2, MG-10b S5): Start or join (`sh-start`), the coordinator's setup
// wizard (`sh-setup`, `sh-terms`), and the session sheet (`sh-session`, `sh-join`, `sh-close`).
// Every session screen renders the engine's `next` (D2); Discard shows only while the engine says
// the session is `discardable`; Abandon only while the engine says it is `abandonable`. Sign, Send
// and Send again confirm the exact `debit` and take a `masternodeOp{debit}` grant, where this network
// sends (`sh-sign-confirm`, `sh-send-confirm`, `sh-resend-confirm`).

import { useEffect, useId, useRef, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { SharedImport } from "../../api/generated/SharedImport";
import type { SharedSession } from "../../api/generated/SharedSession";
import type { UnreadableSession } from "../../api/generated/UnreadableSession";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend, useSyncStatus } from "../../api/queries";
import { saveText } from "../../app/files";
import { navigate } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { showToast } from "../../components/toast";
import { chainOf, useDisplay } from "../../format/display";
import { shortDate } from "../../governance/model";
import { useT } from "../../i18n";
import { useMasternodesRefresh, useSharedCapability, useSharedSession, useSharedSessions, useSharedUnreadable } from "../../masternodes/queries";
import { SHARED_ABANDON_FEE } from "../../api/generated/commands";
import {
  abandonNeedsOwnFirst,
  basisPoints,
  canAbandon,
  DEFAULT_FEE,
  MAX_SHARED_FEE,
  fundingLine,
  grantCap,
  handoffLine,
  joinEstimate,
  mainnetHoldsBack,
  renderSession,
  reservationNote,
  type SendAction,
  sendOffer,
  sessionsInProgress,
  sharedErrorText,
  signedOutText,
  sliceFor,
  stageStep,
  termsDiff,
  unreadableInProgress,
  warningText,
  wayOutShown,
} from "../../masternodes/shared";
import { sendErrorText } from "../pay/model";
import { CredentialField, useCredential } from "./credential";
import { MoreMenu, Notice } from "./common";
import { SharedSetup } from "./SharedSetup";

const P = "masternodes.shared";
type ImportResult = Extract<SharedImport, { kind: "session" }>;

/** What the Start/Join sheet hands on: a session to open, with the import that opened it. */
export interface OpenSession {
  id: string;
  imported: ImportResult | null;
}

export function useSharedText(network: NetworkInfo) {
  const shown = useDisplay(network);
  const dash = (d: number) => shown.withUnit(d);
  const t = useT();
  return { shown, dash, networkName: network.kind === "devnet" ? network.dirName : t(`network.${network.kind}`) };
}

/** `sh-start`: start one, join or continue, sessions in progress (§2.1). */
export function SharedStartSheet({ wallet, network, onClose, onOpen }: { wallet: WalletSummary; network: NetworkInfo; onClose: () => void; onOpen: (s: OpenSession) => void }) {
  const t = useT();
  const b = useBackend();
  const refresh = useMasternodesRefresh();
  const { dash, networkName } = useSharedText(network);
  const capability = useSharedCapability(wallet.walletId);
  const sessions = useSharedSessions(wallet.walletId);
  const unreadable = useSharedUnreadable(wallet.walletId);
  const [discarding, setDiscarding] = useState<UnreadableSession | null>(null);
  const [abandoning, setAbandoning] = useState<UnreadableSession | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [setup, setSetup] = useState(false);
  const pasteId = useId();
  const paste = useRef<HTMLTextAreaElement>(null);
  useEffect(() => paste.current?.focus(), []);

  const v24 = capability.data?.v24 === "active";
  const startOff = wallet.watchOnly ? t(`${P}.watchOnly`) : capability.data && !v24 ? t(`${P}.capabilityUnknown`, { network: networkName }) : null;

  /** Routes each import by slice (D14): a registration opens its sheet; the rest read "saved". */
  const route = (results: SharedImport[]) => {
    const lines: string[] = [];
    for (const r of results) {
      if (r.kind === "standby") {
        lines.push(t(`${P}.savedReadOnly`));
        lines.push(r.stored ? t(`${P}.standbySaved`) : t(`${P}.standbyHeld`));
      } else if (r.session.kind !== "registration") lines.push(t(`${P}.savedReadOnly`));
      else return onOpen({ id: r.session.id, imported: r });
    }
    setNotes(lines);
  };
  const run = async (f: () => Promise<SharedImport[] | null>) => {
    setBusy(true);
    setError(null);
    setNotes([]);
    try {
      const out = await f();
      if (out) route(out);
    } catch (e) {
      setError(sharedErrorText(e, { network: networkName, dash }));
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  if (setup)
    return (
      <SharedSetup
        wallet={wallet}
        network={network}
        onClose={() => setSetup(false)}
        onCreated={(id) => onOpen({ id, imported: null })}
      />
    );

  const inProgress = sessionsInProgress(sessions.data);
  const lost = unreadableInProgress(unreadable.data);
  if (discarding) return <DiscardUnreadable wallet={wallet} network={network} session={discarding} onDone={() => setDiscarding(null)} />;
  if (abandoning)
    return (
      <AbandonSheet
        wallet={wallet}
        network={network}
        code={abandoning.code}
        testId="sh-unreadable-abandon-confirm"
        abandon={(grant) => b.mnSharedAbandonUnreadable(wallet.walletId, abandoning.id, grant)}
        onDone={() => setAbandoning(null)}
      />
    );
  return (
    <Sheet title={t(`${P}.title`)} width="large" testId="sh-start" onClose={onClose}>
      <div className="mg-sheet-body mg-step">
        <p className="mg-muted">{t(`${P}.intro`)}</p>
        <section className="card-l1 sh-choice" data-testid="sh-start-one">
          <h3>{t(`${P}.startTitle`)}</h3>
          <p className="mg-muted">{t(`${P}.startText`)}</p>
          {startOff && (
            <p className="mg-muted mg-small" data-testid="sh-start-off">
              {startOff}
            </p>
          )}
          <DashButton variant="filledBlue" size="small" data-testid="sh-start-button" disabled={!!startOff || !capability.data} onClick={() => setSetup(true)}>
            {t(`${P}.startButton`)}
          </DashButton>
        </section>
        <section className="card-l1 sh-choice" data-testid="sh-join">
          <h3>{t(`${P}.joinTitle`)}</h3>
          <p className="mg-muted">{t(`${P}.joinText`)}</p>
          <div className="form-field">
            <label htmlFor={pasteId}>{t(`${P}.pasteLabel`)}</label>
            <textarea
              ref={paste}
              id={pasteId}
              className="mg-json"
              rows={4}
              spellCheck={false}
              data-testid="sh-paste"
              value={text}
              disabled={busy || wallet.watchOnly}
              onChange={(e) => setText(e.target.value)}
            />
          </div>
          <p className="form-problem" role="alert" data-testid="sh-import-error">
            {error}
          </p>
          {notes.length > 0 && (
            <p className="mg-muted" role="status" data-testid="sh-import-note">
              {notes.join(" ")}
            </p>
          )}
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="small" data-testid="sh-open-file" disabled={busy || wallet.watchOnly} onClick={() => void run(() => b.mnSharedImportFile(wallet.walletId, null))}>
              {t(`${P}.openFile`)}
            </DashButton>
            <DashButton
              variant="filledBlue"
              size="small"
              data-testid="sh-read"
              disabled={busy || wallet.watchOnly || text.trim() === ""}
              onClick={() => void run(async () => [await b.mnSharedImport(wallet.walletId, text, null)])}
            >
              {t(`${P}.readPasted`)}
            </DashButton>
          </span>
        </section>
        <dl className="mg-facts" data-testid="sh-facts">
          <dt>{t(`${P}.facts.shares`)}</dt>
          <dd>{t(`${P}.facts.sharesText`)}</dd>
          <dt>{t(`${P}.facts.coins`)}</dt>
          <dd>{t(`${P}.facts.coinsText`)}</dd>
          <dt>{t(`${P}.facts.leaving`)}</dt>
          <dd>{t(`${P}.facts.leavingText`)}</dd>
        </dl>
        {inProgress.length + lost.length > 0 && (
          <section data-testid="sh-in-progress">
            <h3>{t(`${P}.inProgress`)}</h3>
            <ul className="mg-list">
              {inProgress.map((s) => (
                <li key={s.id}>
                  <button type="button" className="mg-row mg-row-wide" data-testid={`sh-session-${s.code}`} onClick={() => onOpen({ id: s.id, imported: null })}>
                    <span className="mg-row-text">
                      <span className="mg-node-name">{t(`${P}.sessionRow`, { code: s.code })}</span>
                      <span className="mg-node-sub">
                        {t(`${P}.role.${s.role}`)} · {t(`${P}.sessionStarted`, { date: shortDate(s.createdAt) })}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
              {lost.map((s) => (
                <li key={s.id} className="mg-row mg-row-wide sh-unreadable" data-testid={`sh-unreadable-${s.code}`}>
                  <span className="mg-row-text">
                    <span className="mg-node-name">{t(`${P}.unreadable.title`, { code: s.code })}</span>
                    <span className="mg-node-sub">{t(s.discardable ? `${P}.unreadable.discardable` : `${P}.unreadable.mayBeOut`)}</span>
                  </span>
                  {s.discardable && (
                    <DashButton variant="plainRed" size="small" data-testid={`sh-unreadable-discard-${s.code}`} disabled={wallet.watchOnly} onClick={() => setDiscarding(s)}>
                      {t(`${P}.unreadable.discard`)}
                    </DashButton>
                  )}
                  {s.abandonable && (
                    <DashButton variant="plainRed" size="small" data-testid={`sh-unreadable-abandon-${s.code}`} disabled={wallet.watchOnly} onClick={() => setAbandoning(s)}>
                      {t(`${P}.session.abandon`)}
                    </DashButton>
                  )}
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </Sheet>
  );
}

/** Discard of a session this wallet can't read: the same question as a readable one, without amounts it can't know. */
function DiscardUnreadable({ wallet, network, session, onDone }: { wallet: WalletSummary; network: NetworkInfo; session: UnreadableSession; onDone: () => void }) {
  const t = useT();
  const { dash, networkName } = useSharedText(network);
  const b = useBackend();
  const refresh = useMasternodesRefresh();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const discard = async () => {
    setBusy(true);
    try {
      await b.mnSharedDiscard(wallet.walletId, session.id);
      onDone();
    } catch (e) {
      setError(sharedErrorText(e, { network: networkName, dash }));
    } finally {
      setBusy(false);
      void refresh();
    }
  };
  return (
    <Sheet title={t(`${P}.unreadable.title`, { code: session.code })} width="small" testId="sh-unreadable-confirm" onClose={onDone}>
      <div className="mg-sheet-body mg-step">
        <p>{t(`${P}.unreadable.discardable`)}</p>
        <p className="form-problem" role="alert" data-testid="sh-unreadable-error">
          {error}
        </p>
        <footer className="mg-sheet-footer">
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" data-testid="sh-unreadable-cancel" data-autofocus onClick={onDone}>
              {t(`${P}.close.cancel`)}
            </DashButton>
            <DashButton variant="plainRed" size="medium" data-testid="sh-unreadable-confirm-discard" disabled={busy} onClick={() => void discard()}>
              {t(`${P}.unreadable.discard`)}
            </DashButton>
          </span>
        </footer>
      </div>
    </Sheet>
  );
}

/** Abandon's confirm (§9.3, MG-10 §2.3.3): the exact fee, "can't be undone" ticked, the
 * passphrase, then one call. The grant's cap is that fee, so the call can charge nothing more. */
function AbandonSheet({ wallet, network, code, testId, abandon, onDone }: { wallet: WalletSummary; network: NetworkInfo; code: string; testId: string; abandon: (grant: string) => Promise<unknown>; onDone: () => void }) {
  const t = useT();
  const { dash, networkName } = useSharedText(network);
  const refresh = useMasternodesRefresh();
  const credential = useCredential(wallet.walletId, t(`${P}.abandon.ask`), null);
  const [busy, setBusy] = useState(false);
  const [understood, setUnderstood] = useState(false);
  const [ownFirst, setOwnFirst] = useState(false);
  const acting = useRef(false);
  const run = async () => {
    if (acting.current || !credential.ready || !understood) return;
    acting.current = true;
    setBusy(true);
    setOwnFirst(false);
    try {
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: SHARED_ABANDON_FEE });
      if (!grant) return;
      await abandon(grant.id);
      onDone();
    } catch (e) {
      if (abandonNeedsOwnFirst(e)) setOwnFirst(true);
      else credential.setError(sharedErrorText(e, { network: networkName, dash }));
    } finally {
      acting.current = false;
      setBusy(false);
      void refresh();
    }
  };
  return (
    <Sheet title={t(`${P}.session.title`, { code })} width="small" testId={testId} closable={!busy} onClose={onDone}>
      <form
        className="mg-sheet-body mg-step"
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <p data-testid={`${testId}-text`}>{t(`${P}.abandon.question`, { charge: dash(SHARED_ABANDON_FEE) })}</p>
        <p className="mg-muted">{t(`${P}.abandon.after`)}</p>
        <label className="mg-choice">
          <input type="checkbox" data-testid={`${testId}-understand`} checked={understood} disabled={busy} onChange={(e) => setUnderstood(e.target.checked)} />
          <span className="mg-choice-text">{t(`${P}.abandon.understand`)}</span>
        </label>
        <CredentialField c={credential} busy={busy} testId={testId} />
        {ownFirst && (
          <div className="mg-notice mg-notice-info" role="alert" data-testid={`${testId}-own-first`}>
            <span>{t(`${P}.abandon.ownFirst`)}</span>
            <DashButton variant="tintedGray" size="small" data-testid={`${testId}-open-activity`} onClick={() => navigate("activity")}>
              {t(`${P}.abandon.openActivity`)}
            </DashButton>
          </div>
        )}
        <footer className="mg-sheet-footer">
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" data-testid={`${testId}-cancel`} data-autofocus disabled={busy} onClick={onDone}>
              {t(`${P}.close.cancel`)}
            </DashButton>
            <DashButton type="submit" variant="plainRed" size="medium" data-testid={`${testId}-abandon`} disabled={busy || !understood || !credential.ready || wallet.watchOnly}>
              {t(`${P}.session.abandon`)}
            </DashButton>
          </span>
        </footer>
      </form>
    </Sheet>
  );
}

const STAGES = ["invite", "collect", "lock", "approvals", "signatures", "send"] as const;

function StageStrip({ s }: { s: SharedSession }) {
  const t = useT();
  const at = stageStep(s);
  return (
    <ol className="sh-stages" aria-label={t(`${P}.stages.label`)} aria-live="polite" data-testid="sh-stages">
      {STAGES.map((k, i) => (
        <li key={k} className={i + 1 < at ? "sh-stage-done" : i + 1 === at ? "sh-stage-now" : undefined} aria-current={i + 1 === at ? "step" : undefined}>
          {i + 1} {t(`${P}.stages.${k}`)}
        </li>
      ))}
    </ol>
  );
}

function rowChip(s: SharedSession, r: SharedSession["rows"][number], t: Tr): string {
  if (r.fundingSigned) return t(`${P}.session.chip.signed`);
  if (r.approved) return t(`${P}.session.chip.approved`);
  if (r.contributed && s.stage.kind === "draft") return t(`${P}.session.chip.contributed`);
  return t(`${P}.session.chip.waiting`);
}

/** `sh-session`: one session, rendered from `next` (§2.3, §2.3.1). */
export function SharedSessionSheet({ wallet, network, open, onClose }: { wallet: WalletSummary; network: NetworkInfo; open: OpenSession; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const refresh = useMasternodesRefresh();
  const { dash, networkName } = useSharedText(network);
  const query = useSharedSession(wallet.walletId, open.id);
  /** The record a command returned, shown at once; the event re-reads the rest (§3.5). */
  const [latest, setLatest] = useState<SharedSession | null>(open.imported?.session ?? null);
  const [imported, setImported] = useState<ImportResult | null>(open.imported);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reply, setReply] = useState("");
  const [closing, setClosing] = useState(false);
  const [feeOpen, setFeeOpen] = useState(false);
  const [unlocking, setUnlocking] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [pickedShare, setPickedShare] = useState<number | null>(null);
  const [confirming, setConfirming] = useState<SendAction | null>(null);
  const [wayOut, setWayOut] = useState(false);
  const replyId = useId();
  const s = pick(query.data, latest);
  const debit = s?.debit ?? 0;
  const credential = useCredential(wallet.walletId, t(`${P}.session.ask`), 0);
  const sync = useSyncStatus();
  /** A hand-off needs SPV running with a peer (§2.3.3); signing a contribution doesn't. */
  const disconnected = !!sync.data && (!sync.data.running || sync.data.connectedPeers === 0);

  if (!s)
    return (
      <Sheet title={t(`${P}.title`)} width="large" testId="sh-session" onClose={onClose}>
        <div className="mg-sheet-body mg-step" data-testid="sh-session-state">
          {query.isError ? (
            <>
              <p className="form-problem" role="alert">
                {t(`${P}.session.readFailed`, { reason: sharedErrorText(query.error, { network: networkName, session: null, dash }) })}
              </p>
              <span className="mg-actions">
                <DashButton variant="tintedGray" size="medium" data-testid="sh-session-retry" onClick={() => void query.refetch()}>
                  {t(`${P}.session.retry`)}
                </DashButton>
                <DashButton variant="filledBlue" size="medium" data-testid="sh-session-close" onClick={onClose}>
                  {t(`${P}.close.cancel`)}
                </DashButton>
              </span>
            </>
          ) : (
            <p className="mg-muted" aria-live="polite">
              {t(`${P}.session.loading`)}
            </p>
          )}
        </div>
      </Sheet>
    );
  const v = renderSession(s, sliceFor(network));
  const offered = sendOffer(s, network, wallet.watchOnly);
  const handoffHeld = disconnected && (offered === "send" || offered === "resend");
  const mainnetOff = mainnetHoldsBack(s, network);
  const showWayOut = wayOutShown(s, v.row);
  const handoff = handoffLine(s, (code) => sendErrorText({ code, message: "", data: null }, chainOf(network))?.text ?? null);
  const openShares = s.rows.filter((r) => !r.ownerAddress);
  const joinShare = openShares.length === 1 ? openShares[0]!.index : pickedShare;
  const joinRow = s.rows.find((r) => r.index === joinShare) ?? null;
  const ctx = { network: networkName, session: s, dash };
  const coordinatorLabel = s.rows[0]?.label ?? "";
  const act = async (f: () => Promise<{ session: SharedSession }>) => {
    setBusy(true);
    setError(null);
    try {
      const out = await f();
      setLatest(out.session);
    } catch (e) {
      setError(sharedErrorText(e, ctx));
    } finally {
      setBusy(false);
      void refresh();
    }
  };
  const signConsent = (call: (grant: string) => Promise<{ session: SharedSession }>) =>
    act(async () => {
      const cap = grantCap(v.primary === "lock" ? "lock" : "approve", s) ?? 0;
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: cap });
      if (!grant) return { session: s };
      return call(grant.id);
    });
  const copyMessage = async () => {
    try {
      const m = await b.mnSharedMessage(wallet.walletId, s.id);
      await navigator.clipboard.writeText(m.text);
      showToast(t(`${P}.session.copied`));
    } catch (e) {
      setError(sharedErrorText(e, ctx));
    }
  };
  const saveMessage = async () => {
    try {
      const m = await b.mnSharedMessage(wallet.walletId, s.id);
      await saveText(b, "sharedMasternodeMessage", m.fileName, m.text);
    } catch (e) {
      setError(sharedErrorText(e, ctx));
    }
  };
  const pasteReply = () =>
    act(async () => {
      const r = await b.mnSharedImport(wallet.walletId, reply, s.id);
      setReply("");
      if (r.kind === "session") {
        if (r.session.id !== s.id) {
          setError(t(`${P}.error.other_session`));
          return { session: s };
        }
        setImported(r);
        return { session: r.session };
      }
      return { session: s };
    });
  const close = () => (s.discardable ? setClosing(true) : onClose());

  const funding = fundingLine(s, dash);
  const diff = v.row === "approve" ? termsDiff(s.approvedTerms, s.terms, dash) : [];
  const sessionWarnings = s.warnings.map((w) => warningText(w, s)).filter((x): x is string => !!x);
  const importWarnings = imported ? imported.warnings.map((w) => warningText(w, s)).filter((x): x is string => !!x) : [];
  const counts = { joined: s.rows.filter((r) => r.contributed).length, approved: s.rows.filter((r) => r.approved).length, signed: s.rows.filter((r) => r.fundingSigned).length, total: s.rows.length };
  const rowText = rowCopy(v.row, s, { coordinator: coordinatorLabel, dash, funding, counts, t });
  const moreItems = [
    ...(s.role === "coordinator" && s.stage.kind === "draft" && s.rows.slice(1).every((r) => !r.contributed) ? [{ id: "fee", title: t(`${P}.session.changeFee`), onSelect: () => setFeeOpen(true) }] : []),
    ...(s.role === "coordinator" && s.stage.kind === "signing" ? [{ id: "unlock", title: t(`${P}.session.unlock`), onSelect: () => setUnlocking(true) }] : []),
  ];

  if (closing && s.discardable)
    return (
      <Sheet title={t(`${P}.close.title`)} width="small" testId="sh-close" onClose={() => setClosing(false)}>
        <div className="mg-sheet-body mg-step">
          <p>{t(`${P}.close.text`, { debit: dash(debit) })}</p>
          <p className="form-problem" role="alert" data-testid="sh-close-error">
            {error}
          </p>
          <footer className="mg-sheet-footer">
            <DashButton variant="tintedGray" size="medium" data-testid="sh-close-cancel" onClick={() => setClosing(false)}>
              {t(`${P}.close.cancel`)}
            </DashButton>
            <span className="mg-actions">
              <DashButton
                variant="plainRed"
                size="medium"
                data-testid="sh-close-discard"
                disabled={busy}
                onClick={() =>
                  void (async () => {
                    setBusy(true);
                    try {
                      await b.mnSharedDiscard(wallet.walletId, s.id);
                      onClose();
                    } catch (e) {
                      setError(sharedErrorText(e, ctx));
                    } finally {
                      setBusy(false);
                      void refresh();
                    }
                  })()
                }
              >
                {t(`${P}.close.discard`, { debit: dash(debit) })}
              </DashButton>
              <DashButton variant="filledBlue" size="medium" data-testid="sh-close-keep" data-autofocus onClick={onClose}>
                {t(`${P}.close.keep`)}
              </DashButton>
            </span>
          </footer>
        </div>
      </Sheet>
    );

  if (unlocking)
    return (
      <Sheet title={t(`${P}.session.unlock`)} width="small" testId="sh-unlock" onClose={() => setUnlocking(false)}>
        <div className="mg-sheet-body mg-step">
          <p>{t(`${P}.session.unlockConfirm`)}</p>
          <footer className="mg-sheet-footer">
            <span className="mg-actions">
              <DashButton variant="tintedGray" size="medium" data-testid="sh-unlock-cancel" data-autofocus onClick={() => setUnlocking(false)}>
                {t(`${P}.close.cancel`)}
              </DashButton>
              <DashButton
                variant="plainRed"
                size="medium"
                data-testid="sh-unlock-confirm"
                disabled={busy}
                onClick={() => {
                  setUnlocking(false);
                  void act(() => b.mnSharedUnlock(wallet.walletId, s.id));
                }}
              >
                {t(`${P}.session.unlock`)}
              </DashButton>
            </span>
          </footer>
        </div>
      </Sheet>
    );

  if (abandoning)
    return (
      <AbandonSheet
        wallet={wallet}
        network={network}
        code={s.code}
        testId="sh-abandon"
        abandon={async (grant) => setLatest(await b.mnSharedAbandon(wallet.walletId, s.id, grant))}
        onDone={() => setAbandoning(false)}
      />
    );

  if (confirming && confirming === offered)
    return (
      <SendConfirm
        wallet={wallet}
        network={network}
        session={s}
        action={confirming}
        held={handoffHeld}
        onDone={(next) => {
          if (next) setLatest(next);
          setConfirming(null);
        }}
      />
    );

  if (feeOpen) return <FeeSheet wallet={wallet} network={network} session={s} onDone={(next) => (next && setLatest(next), setFeeOpen(false))} />;

  return (
    <Sheet title={t(`${P}.session.title`, { code: s.code })} width="large" testId="sh-session" onClose={close}>
      <div className="mg-sheet-body mg-step">
        <StageStrip s={s} />
        <p className="mg-muted mg-small" data-testid="sh-codes">
          {t(`${P}.session.checkCode`, { code: s.messageCode })} · {t(`${P}.session.sessionCode`, { code: s.code })}
        </p>
        <table className="sh-participants" data-testid="sh-participants">
          <thead>
            <tr>
              <th>{t(`${P}.session.participant`)}</th>
              <th>{t(`${P}.session.share`)}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {s.rows.map((r) => (
              <tr key={r.index} data-testid={`sh-row-${r.index}`}>
                <td>
                  {r.label}
                  {r.mine && <span className="mg-muted"> ({t(`${P}.session.you`)})</span>}
                  {r.index === 0 && <span className="mg-node-sub">{t(`${P}.session.coordinator`)}</span>}
                </td>
                <td>{dash(r.amount)}</td>
                <td>{rowChip(s, r, t)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <dl className="mg-facts" data-testid="sh-terms">
          <dt>{t(`${P}.session.service`)}</dt>
          <dd>
            {s.terms.service}
            {s.terms.serviceRead && <span className="mg-node-sub">{t(`${P}.session.serviceRead`, { read: s.terms.serviceRead })}</span>}
          </dd>
          <dt>{t(`${P}.session.operator`)}</dt>
          <dd className="mono">{shortKey(s.terms.operatorPublicKey)}</dd>
          <dt>{t(`${P}.session.operatorShare`)}</dt>
          <dd>{t(`${P}.session.operatorShareValue`, { share: basisPoints(s.terms.operatorReward) })}</dd>
          <dt>{t(`${P}.session.voting`)}</dt>
          <dd className="mono">{s.terms.votingAddress}</dd>
          {s.nodeRunBy && (
            <>
              <dt>{t(`${P}.session.runBy`)}</dt>
              <dd>{s.nodeRunBy}</dd>
            </>
          )}
          <dt>{t(`${P}.session.early`)}</dt>
          <dd>{s.terms.earlyPeriodBlocks > 0 ? t(`${P}.session.earlyValue`, { blocks: s.terms.earlyPeriodBlocks, penalty: dash(s.terms.earlyPenalty) }) : t(`${P}.session.earlyNone`)}</dd>
          {s.fee !== null && (
            <>
              <dt>{t(`${P}.session.fee`)}</dt>
              <dd>{s.role === "coordinator" ? t(`${P}.funding.feeYours`, { fee: dash(s.fee) }) : t(`${P}.funding.payer`, { coordinator: coordinatorLabel })}</dd>
            </>
          )}
        </dl>
        {sessionWarnings.map((w) => (
          <Notice key={w} tone="warning" testId="sh-warning">
            {w}
          </Notice>
        ))}
        {diff.length > 0 && (
          <Notice tone="warning" testId="sh-diff">
            {t(`${P}.row.changedSinceApproved`, { changes: diff.map((d) => `${d.before} → ${d.after}`).join(", ") })}
          </Notice>
        )}
        <p className="sh-row-copy" aria-live="polite" data-testid={`sh-row-${v.row}`}>
          {rowText}
        </p>
        {v.row === "join" && openShares.length > 1 && (
          <div className="mg-choices" role="radiogroup" aria-label={t(`${P}.session.pickShare`)} data-testid="sh-join-pick">
            <p className="mg-muted mg-small">{t(`${P}.session.pickShare`)}</p>
            {openShares.map((r) => (
              <label key={r.index} className="mg-choice" data-testid={`sh-join-pick-${r.index}`}>
                <input type="radio" name={`${replyId}-share`} checked={pickedShare === r.index} onChange={() => setPickedShare(r.index)} />
                <span className="mg-choice-text">{t(`${P}.session.pickShareOption`, { label: r.label, amount: dash(r.amount) })}</span>
              </label>
            ))}
          </div>
        )}
        {v.row === "join" && joinShare !== null && <p className="mg-muted">{joinEstimate(s, joinShare, dash)}</p>}
        {v.primary === "join" && (
          <p className="mg-muted mg-small" data-testid="sh-reservation-note">
            {reservationNote(network)}
          </p>
        )}
        {handoff && (
          <p className="mg-muted" data-testid="sh-handoff">
            {handoff}
          </p>
        )}
        {showWayOut && (
          <div data-testid="sh-way-out">
            <DashButton variant="plainBlue" size="small" aria-expanded={wayOut} data-testid="sh-way-out-toggle" onClick={() => setWayOut((o) => !o)}>
              {t(`${P}.wayOut.title`)}
            </DashButton>
            {wayOut && (
              <p className="mg-muted mg-small" data-testid="sh-way-out-text">
                {t(`${P}.abandon.ownFirst`)}{" "}
                <DashButton
                  variant="plainBlue"
                  size="small"
                  data-testid="sh-way-out-activity"
                  onClick={() => {
                    onClose();
                    navigate("activity", { activityFilter: { kind: "txids", title: t(`${P}.sessionRow`, { code: s.code }), txids: [s.txid!] } });
                  }}
                >
                  {t(`${P}.abandon.openActivity`)}
                </DashButton>
                {canAbandon(s) && (
                  <DashButton variant="plainRed" size="small" data-testid="sh-way-out-abandon" disabled={busy || wallet.watchOnly} onClick={() => setAbandoning(true)}>
                    {t(`${P}.session.abandon`)}
                  </DashButton>
                )}
              </p>
            )}
          </div>
        )}
        {handoffHeld && (
          <p className="mg-muted mg-small" data-testid="sh-spv-stopped">
            {t(`${P}.row.spvStopped`)}
          </p>
        )}
        {mainnetOff && (
          <Notice tone="info" testId="sh-mainnet-note">
            {t(`${P}.mainnetSignNote`)}
          </Notice>
        )}
        {imported && (
          <div className="mg-notice mg-notice-info" data-testid="sh-import-panel">
            <span>
              {t(`${P}.importPanel.received`, { code: imported.receivedCode })}
              {importWarnings.map((w) => (
                <span key={w} className="sh-import-warning">
                  {" "}
                  {w}
                </span>
              ))}
            </span>
            <DashButton variant="tintedGray" size="small" data-testid="sh-import-dismiss" onClick={() => setImported(null)}>
              {t(`${P}.importPanel.dismiss`)}
            </DashButton>
          </div>
        )}
        {s.next !== "join" && s.stage.kind !== "confirmed" && s.stage.kind !== "dead" && (
          <div className="form-field">
            <label htmlFor={replyId}>{t(`${P}.session.pasteReply`)}</label>
            <textarea id={replyId} className="settings-textarea" rows={3} spellCheck={false} data-testid="sh-reply" value={reply} disabled={busy} onChange={(e) => setReply(e.target.value)} />
            <DashButton variant="tintedGray" size="small" data-testid="sh-reply-read" disabled={busy || reply.trim() === ""} onClick={() => void pasteReply()}>
              {t(`${P}.readPasted`)}
            </DashButton>
          </div>
        )}
        {(v.primary === "lock" || v.primary === "approve") && <CredentialField c={credential} busy={busy} testId="sh-credential" />}
        <p className="form-problem" role="alert" data-testid="sh-error">
          {error}
        </p>
        <footer className="mg-sheet-footer">
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" data-testid="sh-close-button" onClick={close}>
              {t(`${P}.session.close`)}
            </DashButton>
            {moreItems.length > 0 && <MoreMenu testId="sh-more" items={moreItems} />}
            {canAbandon(s) && (
              <DashButton variant="plainRed" size="medium" data-testid="sh-abandon-button" disabled={busy || wallet.watchOnly} onClick={() => setAbandoning(true)}>
                {t(`${P}.session.abandon`)}
              </DashButton>
            )}
          </span>
          <span className="mg-actions" aria-live="polite">
            {v.copy && (
              <>
                <DashButton variant="tintedGray" size="medium" data-testid="sh-save" onClick={() => void saveMessage()}>
                  {t(`${P}.session.saveFile`)}
                </DashButton>
                <DashButton variant="filledBlue" size="medium" data-testid="sh-copy" onClick={() => void copyMessage()}>
                  {copyLabel(v.row, t)}
                </DashButton>
              </>
            )}
            {v.primary === "join" && (
              <DashButton
                variant="filledBlue"
                size="medium"
                data-testid="sh-join-button"
                disabled={busy || wallet.watchOnly || joinShare === null}
                onClick={() => void act(() => b.mnSharedJoin(wallet.walletId, s.id, { shareIndex: joinShare, reward: { kind: "refund" } }))}
              >
                {t(`${P}.row.joinButton`, { amount: dash(joinRow?.amount ?? 0) })}
              </DashButton>
            )}
            {v.primary === "lock" && (
              <DashButton variant="filledBlue" size="medium" data-testid="sh-lock" disabled={busy || !credential.ready || wallet.watchOnly} onClick={() => void signConsent((g) => b.mnSharedLock(wallet.walletId, s.id, s.revision, g))}>
                {t(`${P}.row.lockButton`)}
              </DashButton>
            )}
            {v.primary === "approve" && (
              <DashButton
                variant="filledBlue"
                size="medium"
                data-testid="sh-approve"
                disabled={busy || !credential.ready || wallet.watchOnly || !s.consentHash}
                onClick={() => void signConsent((g) => b.mnSharedApprove(wallet.walletId, s.id, s.consentHash!, g))}
              >
                {t(`${P}.row.approveButton`)}
              </DashButton>
            )}
            {offered && (
              <DashButton variant={offered === "resend" ? "tintedGray" : "filledBlue"} size="medium" data-testid={`sh-${offered}`} disabled={busy || handoffHeld} onClick={() => setConfirming(offered)}>
                {t(SEND_LABEL[offered])}
              </DashButton>
            )}
          </span>
        </footer>
      </div>
    </Sheet>
  );
}

/** The newer of the query's record and the one a command returned (D4: the copy on screen is
 * the latest the wallet holds). */
function pick(a: SharedSession | undefined, b: SharedSession | null): SharedSession | null {
  if (!a) return b;
  if (!b) return a;
  return b.updatedAt > a.updatedAt || (b.updatedAt === a.updatedAt && b.revision > a.revision) ? b : a;
}

const shortKey = (k: string) => (k.length > 12 ? `${k.slice(0, 4)}…${k.slice(-4)}` : k);

type Tr = ReturnType<typeof useT>;

function copyLabel(row: string, t: Tr): string {
  switch (row) {
    case "waitingReplies":
      return t(`${P}.row.copyInvitation`);
    case "sendReply":
      return t(`${P}.row.copyReply`);
    case "sendApproval":
      return t(`${P}.row.copyApproval`);
    case "approvedCount":
      return t(`${P}.row.copyLocked`);
    case "sendSigned":
      return t(`${P}.row.copySigned`);
    case "waitingSigned":
      return t(`${P}.row.copySigning`);
    default:
      return t(`${P}.row.copyInvitation`);
  }
}

function rowCopy(
  row: string,
  s: SharedSession,
  x: { coordinator: string; dash: (d: number) => string; funding: string | null; counts: { joined: number; approved: number; signed: number; total: number }; t: Tr },
): string {
  const { t, coordinator, dash, funding, counts } = x;
  const debit = dash(s.debit ?? 0);
  const others = Math.max(0, counts.total - 1);
  switch (row) {
    case "join":
      return t(`${P}.row.join`);
    case "waitingReplies":
      return t(`${P}.row.waitingReplies`, { joined: Math.max(0, counts.joined - 1), total: others });
    case "sendReply":
      return [t(`${P}.row.sendReply`, { coordinator }), funding].filter(Boolean).join(" ");
    case "lock":
      return [t(`${P}.row.lockText`), funding].filter(Boolean).join(" ");
    case "approve":
      return funding ? t(`${P}.funding.ifSent`, { line: funding }) : "";
    case "sendApproval":
      return t(`${P}.row.sendApproval`, { coordinator });
    case "approvedCount":
      return t(`${P}.row.approvedCount`, { approved: counts.approved, total: counts.total });
    case "sign":
      return s.debit === null ? "" : t(`${P}.row.signText`, { debit, coordinator });
    case "signLater":
      return t(`${P}.row.signLater`);
    case "signedOut":
      return canAbandon(s) ? signedOutText() : "";
    case "sendSigned":
      return t(`${P}.row.sendSigned`, { coordinator });
    case "waitingSigned":
      return t(`${P}.row.waitingSigned`, { signed: counts.signed, total: others });
    case "send":
      return s.debit === null ? "" : t(`${P}.row.everyoneSigned`, { debit });
    case "sent":
      return t(`${P}.row.sent`);
    case "confirmed":
      return t(`${P}.row.confirmed`);
    case "dead":
      return t(`${P}.row.dead`);
    default:
      return "";
  }
}

const SEND_LABEL = { sign: `${P}.row.signButton`, send: `${P}.row.sendButton`, resend: `${P}.row.resendButton` } as const;

/** The confirm before Sign, Send or Send again (§2.3.1, §2.3.3): what leaves this wallet, the exact
 * `debit` that caps the `masternodeOp` grant, the passphrase, then the call. */
function SendConfirm({ wallet, network, session: s, action, held, onDone }: { wallet: WalletSummary; network: NetworkInfo; session: SharedSession; action: SendAction; held: boolean; onDone: (next: SharedSession | null) => void }) {
  const t = useT();
  const b = useBackend();
  const refresh = useMasternodesRefresh();
  const { dash, networkName } = useSharedText(network);
  const cap = grantCap(action, s) ?? 0;
  const ask = t(action === "sign" ? `${P}.row.signAsk` : `${P}.row.sendAsk`);
  // A grant of a share's whole debit always asks on an encrypted vault: the signing window never covers it.
  const credential = useCredential(wallet.walletId, ask, null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const text =
    action === "sign"
      ? t(`${P}.row.signText`, { debit: dash(cap), coordinator: s.rows[0]?.label ?? "" })
      : action === "send"
        ? t(`${P}.row.everyoneSigned`, { debit: dash(cap) })
        : t(`${P}.row.resendText`, { charge: dash(cap) });
  const funding = action === "resend" ? null : fundingLine(s, dash);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: cap });
      if (!grant) return;
      const out = action === "sign" ? await b.mnSharedSign(wallet.walletId, s.id, grant.id) : await b.mnSharedSend(wallet.walletId, s.id, grant.id);
      onDone(out.session);
    } catch (e) {
      setError(sharedErrorText(e, { network: networkName, session: s, dash }));
    } finally {
      setBusy(false);
      void refresh();
    }
  };
  return (
    <Sheet title={t(SEND_LABEL[action])} width="small" testId={`sh-${action}-confirm`} onClose={() => !busy && onDone(null)}>
      <div className="mg-sheet-body mg-step">
        <p data-testid={`sh-${action}-text`}>{text}</p>
        {funding && (
          <p className="mg-muted" data-testid={`sh-${action}-funding`}>
            {funding}
          </p>
        )}
        {credential.prompt === "passphrase" && <p className="mg-muted mg-small">{ask}</p>}
        <CredentialField c={credential} busy={busy} testId={`sh-${action}-credential`} />
        {held && (
          <p className="mg-muted mg-small" data-testid={`sh-${action}-spv-stopped`}>
            {t(`${P}.row.spvStopped`)}
          </p>
        )}
        <p className="form-problem" role="alert" data-testid={`sh-${action}-error`}>
          {error}
        </p>
        <footer className="mg-sheet-footer">
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" data-testid={`sh-${action}-cancel`} disabled={busy} onClick={() => onDone(null)}>
              {t(`${P}.close.cancel`)}
            </DashButton>
            <DashButton variant="filledBlue" size="medium" data-testid={`sh-${action}-go`} disabled={busy || held || !credential.ready || credential.waitSecs > 0} onClick={() => void run()}>
              {t(SEND_LABEL[action])}
            </DashButton>
          </span>
        </footer>
      </div>
    </Sheet>
  );
}

/** More ▸ Change network fee… (§2.2): only in a draft before any reply. */
function FeeSheet({ wallet, network, session, onDone }: { wallet: WalletSummary; network: NetworkInfo; session: SharedSession; onDone: (s: SharedSession | null) => void }) {
  const t = useT();
  const b = useBackend();
  const { dash, networkName } = useSharedText(network);
  const [fee, setFee] = useState(String(session.fee ?? DEFAULT_FEE));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const id = useId();
  const typed = /^\d{1,9}$/.test(fee.trim()) ? Number(fee.trim()) : null;
  const duffs = typed !== null && typed >= 1 && typed <= MAX_SHARED_FEE ? typed : null;
  return (
    <Sheet title={t(`${P}.feeSheet.title`)} width="small" testId="sh-fee" onClose={() => onDone(null)}>
      <div className="mg-sheet-body mg-step">
        <div className="form-field">
          <label htmlFor={id}>{t(`${P}.feeSheet.label`)}</label>
          <input id={id} inputMode="numeric" data-testid="sh-fee-input" value={fee} onChange={(e) => setFee(e.target.value)} />
          {typed !== null && <span className="mg-field-help" data-testid="sh-fee-dash">{dash(typed)}</span>}
          {duffs === null && <span className="mg-field-help">{t(`${P}.feeSheet.range`, { max: MAX_SHARED_FEE.toLocaleString(), maxDash: dash(MAX_SHARED_FEE) })}</span>}
        </div>
        <p className="form-problem" role="alert">
          {error}
        </p>
        <footer className="mg-sheet-footer">
          <span />
          <DashButton
            variant="filledBlue"
            size="medium"
            data-testid="sh-fee-save"
            disabled={busy || duffs === null}
            onClick={() =>
              void (async () => {
                setBusy(true);
                try {
                  onDone((await b.mnSharedSetFee(wallet.walletId, session.id, duffs!)).session);
                } catch (e) {
                  setError(sharedErrorText(e, { network: networkName, session, dash }));
                } finally {
                  setBusy(false);
                }
              })()
            }
          >
            {t(`${P}.feeSheet.save`)}
          </DashButton>
        </footer>
      </div>
    </Sheet>
  );
}

