// Transaction detail sheet (UX-SPEC §4.9, C23 medium): iOS TxDetail header with the dash-qt
// facts (research 02 §4.6), the label editor, and Abandon / Resend where they apply (U-03 D15). With prices
// (MP-01), what the payment was worth when first seen (its stamp, rates-engine §6) and its value now.

import { useId, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage, isAppError } from "../api/backend";
import type { NetworkKind } from "../api/generated/NetworkKind";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxCounterparty } from "../api/generated/TxCounterparty";
import type { TxDetailExtrasInfo } from "../api/generated/TxDetailExtrasInfo";
import type { TxDetails } from "../api/generated/TxDetails";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import type { WalletSummary } from "../api/generated/WalletSummary";
import type { TxFiat } from "../api/generated/TxFiat";
import { keys, useBackend, useTxDetail, useTxDetailExtras } from "../api/queries";
import { navigate, ui, useUi } from "../app/ui";
import { AmountText, DashButton, Icon } from "../components/basics";
import type { UsedIcon } from "../components/icons";
import { LoadingState } from "../components/history";
import { Sheet } from "../components/sheet";
import { CopyValue, DetailRow } from "../components/detail";
import { findContact, subtitle } from "../contacts/model";
import { recordUnreadableText, unreadableWallet } from "../history/recordUnreadable";
import { contactListQuery, useContactsOwner } from "../contacts/queries";
import { FiatLine, timeText, useFiat } from "../fiat/rates";
import { useDisplay } from "../format/display";
import { useT } from "../i18n";
import { explorerUrl } from "../format/explorer";
import { fiatText } from "../format/fiat";
import { t } from "../i18n";
import { Avatar } from "./contacts/Avatar";
import { contactLetters, contactName, detailDate, detailWhat, detailWho, isProblem, rowDate, shownAmount, txIcon } from "../format/tx";

const confirmationsText = (n: number) => `${n} ${n === 1 ? "confirmation" : "confirmations"}`;

/** Plain-language status for the detail sheet; dash-qt's raw string stays in engine/CSV data. The
 * engine keeps `instantLocked` set once a block confirms the transaction, so only an unconfirmed
 * row reads as InstantSend (the activity row's word for it). A count of confirmations is technical
 * and stays in the folded details. */
export function statusText(s: TxStatusInfo): string {
  switch (s.kind) {
    case "abandoned":
      return "Abandoned";
    case "conflicted":
      return "Conflicted";
    case "notAccepted":
      return "Not accepted";
    case "unconfirmed":
      return s.instantLocked ? "InstantSend" : "Waiting for confirmation";
    case "immature":
      return "Maturing";
    case "confirming":
    case "confirmed":
      return "Confirmed";
  }
}

/** Opens the transaction on Insight in the system browser. The app builds the URL from its own
 * network; this only asks, and only when this network has a page. A failure stays on the sheet. */
function ExplorerButton({ txid, kind }: { txid: string; kind: NetworkKind }) {
  const backend = useBackend();
  const [problem, setProblem] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  if (!explorerUrl(kind, txid)) return null;
  return (
    <>
      <button
        type="button"
        className="tx-detail-explorer"
        data-testid="tx-detail-explorer"
        disabled={opening}
        onClick={() => {
          setProblem(null);
          setOpening(true);
          void backend
            .openTxInExplorer(txid)
            .catch(() => setProblem("Could not open the explorer."))
            .finally(() => setOpening(false));
        }}
      >
        {opening ? "Opening the explorer…" : "View in explorer"}
      </button>
      {problem && (
        <p className="field-error" role="alert" data-testid="tx-detail-explorer-error">
          {problem}
        </p>
      )}
    </>
  );
}

/** The confirmation count, for the folded details. Nothing while there are none, so the sheet
 * never says "0" or "unconfirmed". */
export function confirmationDetail(s: TxStatusInfo): string | null {
  if (s.kind !== "confirming" && s.kind !== "confirmed" && s.kind !== "immature") return null;
  if (s.confirmations === 0) return s.kind === "immature" && s.maturesIn !== null ? `Available in ${s.maturesIn} more blocks` : null;
  const conf = confirmationsText(s.confirmations);
  return s.kind === "immature" && s.maturesIn !== null ? `${conf}, available in ${s.maturesIn} more blocks` : conf;
}

const STAMP_SOURCE = { ctx: "rates.ctx.com", bitPayDashCentral: "BitPay and DashCentral" } as const;

/** "$12.34 when received": the amount at the price it was first seen with, in that price's own
 * currency; nothing when it has no stamp. */
function FiatThen({ stamp, duffs, sent, internal, locale }: { stamp: TxFiat; duffs: number; sent: boolean; internal: boolean; locale: string | undefined }) {
  const text = fiatText(duffs, stamp.price, stamp.currency, locale);
  if (text === null) return null;
  const when = internal ? "at the time" : sent ? "when sent" : "when received";
  const date = new Date(stamp.rateAt * 1000).toLocaleDateString(locale, { year: "numeric", month: "short", day: "numeric" });
  return (
    <p className="tx-detail-fiat-then" data-testid="tx-detail-fiat-then" title={`Price from ${date}, ${timeText(stamp.rateAt, locale)}, from ${STAMP_SOURCE[stamp.source]}`}>
      {text} {when}
    </p>
  );
}

/** The transaction's label (QT-093): saved on Save, cleared when empty. */
function LabelEditor({ walletId, txid, label }: { walletId: string; txid: string; label: string | null }) {
  const b = useBackend();
  const client = useQueryClient();
  const id = useId();
  const [text, setText] = useState(label ?? "");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const changed = text.trim() !== (label ?? "");
  const save = async () => {
    setBusy(true);
    setProblem(null);
    try {
      await b.setTxLabel(walletId, txid, text.trim() || null);
      await client.invalidateQueries({ queryKey: keys.historyAll(walletId) });
      await client.invalidateQueries({ queryKey: keys.txDetail(walletId, txid) });
    } catch (e) {
      setProblem(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="label-editor card-l1"
      data-testid="tx-detail-label"
      onSubmit={(e) => {
        e.preventDefault();
        if (changed && !busy) void save();
      }}
    >
      <label htmlFor={id}>Label</label>
      <span className="label-editor-row">
        <input id={id} data-testid="tx-label-input" value={text} autoComplete="off" placeholder="Add a label" onChange={(e) => setText(e.target.value)} />
        <DashButton type="submit" variant="tintedBlue" size="small" data-testid="tx-label-save" disabled={!changed || busy}>
          Save
        </DashButton>
      </span>
      {problem && (
        <span className="field-error" role="alert">
          {problem}
        </span>
      )}
    </form>
  );
}

/** The abandon confirmation. Whether the wallet spent inputs decides it (`totalDebit`, the engine's
 * `from_me`), not the amount's sign: a payment to yourself nets only the fee but spends this wallet's
 * coins. An unknown debit takes the spending text. A debit of none with an input whose coin the
 * engine holds no record of (`inputsUnresolved`) may still be a payment sent from this wallet on
 * another device, so it says what sending it again would do. */
export function abandonQuestion(x: Pick<TxDetailExtrasInfo, "totalDebit" | "inputsUnresolved">): string {
  if (x.totalDebit !== 0) return t("activity.abandon.spending");
  return x.inputsUnresolved ? t("activity.abandon.unknownInputs") : t("activity.abandon.notSpending");
}

/** What a failed Abandon or Resend says: the engine's message, but the steps of a `rescanFirst`
 * refusal, why a `settledDescendant` one can't be abandoned, and which open wallet (of `wallets`)
 * a `recordUnreadable` one names from the catalog. */
export function actionProblem(e: unknown, wallets: readonly WalletSummary[] = []): string {
  const unreadable = unreadableWallet(e);
  if (unreadable !== undefined) return recordUnreadableText(unreadable, wallets);
  if (isAppError(e) && e.data?.kind === "txAction") {
    if (e.data.refusal === "rescanFirst") return t("activity.abandon.rescanFirst");
    if (e.data.refusal === "settledDescendant") return t("activity.abandon.settledChild");
  }
  return errorMessage(e);
}

/** Abandon and Resend (QT-091), shown only when the engine says they apply. */
function Actions({ walletId, txid }: { walletId: string; txid: string }) {
  const b = useBackend();
  const client = useQueryClient();
  const extras = useTxDetailExtras(walletId, txid);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<{ text: string; problem: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const x = extras.data;
  const can = !!x && (x.canAbandon || x.canResend);
  // After an abandon the actions go, but its note stays.
  if (!x || (!can && !note)) return null;
  const run = async (what: "abandon" | "resend") => {
    setBusy(true);
    setNote(null);
    try {
      if (what === "abandon") await b.abandonTransaction(walletId, txid);
      else await b.resendTransaction(walletId, txid);
      setNote({ text: what === "abandon" ? "The transaction was abandoned." : "The transaction was sent to the network again.", problem: false });
      setConfirming(false);
      await client.invalidateQueries({ queryKey: keys.txDetailAll(walletId) });
      await client.invalidateQueries({ queryKey: keys.historyAll(walletId) });
    } catch (e) {
      const wallets = await b.listWallets().catch(() => []);
      setNote({ text: actionProblem(e, wallets), problem: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="tx-actions card-l1" aria-label="Actions" data-testid="tx-detail-actions">
      {confirming ? (
        <>
          <p className="tx-actions-question">
            {abandonQuestion(x)}
          </p>
          <span className="tx-actions-row">
            <DashButton variant="tintedGray" size="small" disabled={busy} onClick={() => setConfirming(false)}>
              Keep it
            </DashButton>
            <DashButton variant="plainRed" size="small" data-testid="tx-abandon-confirm" disabled={busy} onClick={() => void run("abandon")}>
              Abandon transaction
            </DashButton>
          </span>
        </>
      ) : (
        can && (
          <span className="tx-actions-row">
            {x.canResend && (
              <DashButton variant="tintedBlue" size="small" data-testid="tx-resend" disabled={busy} onClick={() => void run("resend")}>
                Resend
              </DashButton>
            )}
            {x.canAbandon && (
              <DashButton variant="plainRed" size="small" data-testid="tx-abandon" disabled={busy} onClick={() => setConfirming(true)}>
                Abandon transaction
              </DashButton>
            )}
          </span>
        )
      )}
      {note && (
        <p className={note.problem ? "form-problem" : "placeholder-note"} role={note.problem ? "alert" : "status"} data-testid="tx-action-note">
          {note.text}
        </p>
      )}
    </section>
  );
}

/** Whether the main identity's Contacts lists `contact` (DP3-07 D4), in any section, from the
 * query Contacts' unfiltered list reads: only then does its pane open on it. */
function useListed(walletId: string, contact: string): boolean {
  const b = useBackend();
  const { owner } = useContactsOwner(walletId);
  const identity = owner.kind === "identity" ? owner.identity : null;
  const sort = useUi((s) => s.contactSort);
  const list = useQuery({ ...contactListQuery(b, walletId, identity ?? "", sort, ""), enabled: identity !== null });
  return identity !== null && findContact(list.data, contact) !== null;
}

/** Where focus goes once View contact closed the sheet, whose opener left with its page (D4): the
 * contact's row, else the Contacts heading. */
function contactsFocus(): HTMLElement | null {
  const page = document.querySelector<HTMLElement>(".app-body > main");
  const row = page?.querySelector<HTMLElement>('[aria-current="true"]');
  if (row) return row;
  const heading = page?.querySelector<HTMLElement>("h1") ?? null;
  if (heading && !heading.hasAttribute("tabindex")) heading.tabIndex = -1;
  return heading;
}

/** The DashPay contact of the record the sheet shows (DP3-02 D24): its avatar, name and
 * `@username` (D6), and View contact, which opens its pane, when Contacts lists it (D4). */
function ContactFact({
  walletId,
  counterparty: c,
  name,
  sent,
  onView,
}: {
  walletId: string;
  counterparty: TxCounterparty;
  name: string;
  sent: boolean;
  onView: () => void;
}) {
  const listed = useListed(walletId, c.identity);
  const sub = subtitle({ alias: null, displayName: c.displayName, username: c.username, contact: c.identity });
  const letters = contactLetters(c);
  return (
    <DetailRow label={sent ? "Sent to contact" : "Received from contact"} testId="tx-detail-contact">
      <span className="tx-detail-contact">
        {letters !== null && <Avatar letters={letters} identity={c.identity} />}
        <span className="tx-detail-contact-names">
          <span className="tx-detail-contact-name">{name}</span>
          {sub && <span className="contact-row-username">{sub}</span>}
        </span>
        {listed && (
          <DashButton
            variant="tintedBlue"
            size="small"
            data-testid="tx-detail-view-contact"
            onClick={() => {
              onView();
              navigate("contacts", { contact: c.identity, addContact: null, contactPrefill: null });
            }}
          >
            View contact
          </DashButton>
        )}
      </span>
    </DetailRow>
  );
}

function Body({ d, network, walletId, onViewContact }: { d: TxDetails; network: NetworkInfo; walletId: string; onViewContact: () => void }) {
  const t = useT();
  const backend = useBackend();
  const row = d.rows[0];
  const amount = d.rows.reduce((sum, r) => sum + r.amount, 0);
  const icon = row ? txIcon(row.txType, d.status, amount) : "received";
  const headerIcon: UsedIcon = icon === "error" ? "txDetailError" : amount < 0 ? "txDetailSent" : "txDetailReceived";
  const date = rowDate(d);
  // The row's address; else, received: our own output, sent: the first output that is not ours.
  const counterparty =
    row?.address ?? d.outputs.find((o) => (amount >= 0 ? o.isMine : !o.isMine) && !o.isChange)?.address ?? null;
  const shown = shownAmount(amount, row?.txType ?? "other");
  const own = d.outputs.some((o) => o.isMine && o.address === counterparty);
  const display = useDisplay(network);
  const unit = display.name;
  // The DashPay contact of the record whose title and address the sheet shows (DP3-02 D24).
  const contact = contactName(row?.counterparty);
  const what = row ? detailWhat({ ...row, amount }) : "Transaction";
  const who = row ? detailWho({ ...row, address: counterparty }) : null;
  const sub =
    contact && who === contact && row?.counterparty
      ? subtitle({ alias: null, displayName: row.counterparty.displayName, username: row.counterparty.username, contact: row.counterparty.identity })
      : null;
  const fiat = useFiat();
  const discreet = useUi((s) => s.discreet);
  const now = discreet ? null : fiat.text(shown.duffs);
  // A stamp is history, shown while prices are on, today's price or not; off hides every value.
  const stamp = !discreet && fiat.view && fiat.view.state.kind !== "off" ? row?.fiatThen : undefined;
  const confirmations = confirmationDetail(d.status);
  return (
    <>
      <div className="tx-detail-summary" data-testid="tx-detail-summary">
        <Icon id={headerIcon} size={50} />
        {who && who !== what && (
          <p className="tx-detail-who" data-testid="tx-detail-who">
            <span className="tx-detail-who-name">{who}</span>
            {sub && <span className="tx-detail-who-user"> {sub}</span>}
          </p>
        )}
        <h3 className="tx-detail-title" data-testid="tx-detail-title">{what}</h3>
        <AmountText className="tx-detail-amount" testId="tx-detail-amount" text={display.full(shown.duffs, shown.signed)} unitName={unit} glyphFactor={0.8} />
        {stamp && <FiatThen stamp={stamp} duffs={shown.duffs} sent={amount < 0} internal={!shown.signed} locale={backend.locale} />}
        <div className="tx-detail-status">
          {date && (
            <span className="tx-detail-date" data-testid="tx-detail-when">
              {detailDate(date, backend.locale)}
            </span>
          )}
          <span className={isProblem(d.status) ? "tx-detail-state is-problem" : "tx-detail-state"} data-testid="tx-detail-status">
            {statusText(d.status)}
          </span>
        </div>
        <ExplorerButton txid={d.txid} kind={network.kind} />
      </div>
      <dl className="detail-list card-l1" data-testid="tx-detail-facts">
        {contact && row?.counterparty && <ContactFact walletId={walletId} counterparty={row.counterparty} name={contact} sent={amount < 0} onView={onViewContact} />}
        {counterparty && (
          <DetailRow label={amount < 0 ? "To" : "Received with"}>
            <CopyValue value={counterparty} label="address" />
            {own && <span className="detail-note"> (own address)</span>}
          </DetailRow>
        )}
        {now !== null && (
          <DetailRow label="Value now" testId="tx-detail-value-now">
            <FiatLine fiat={fiat} duffs={shown.duffs} testId="tx-detail-value-now-text" />
          </DetailRow>
        )}
        {d.message && <DetailRow label="Message">{d.message}</DetailRow>}
      </dl>
      <LabelEditor key={`label-${d.txid}`} walletId={walletId} txid={d.txid} label={d.label} />
      <Actions key={`actions-${d.txid}`} walletId={walletId} txid={d.txid} />
      <details className="disclosure card-l1" data-testid="tx-detail-technical">
        <summary>Details</summary>
        <dl className="detail-list">
          {d.fee !== null && (
            <DetailRow label="Network fee">
              <AmountText text={display.full(d.fee)} unitName={unit} glyph={false} />
            </DetailRow>
          )}
          {confirmations && <DetailRow label="Confirmations">{confirmations}</DetailRow>}
          {d.blockHeight !== null && <DetailRow label="Block">{d.blockHeight.toLocaleString(backend.locale)}</DetailRow>}
          <DetailRow label="Transaction ID" testId="tx-detail-txid">
            <CopyValue value={d.txid} label="transaction ID" mono />
          </DetailRow>
          <DetailRow label="Size">{`${d.sizeBytes} bytes`}</DetailRow>
          {d.rawHex && (
            <DetailRow label={t("activity.detail.rawTransaction")} testId="tx-detail-raw">
              <CopyValue value={d.rawHex} action={t("activity.detail.copyRawTransaction")} mono display={d.rawHex.length > 32 ? `${d.rawHex.slice(0, 32)}…` : d.rawHex} />
            </DetailRow>
          )}
        </dl>
        <p className="io-heading">Inputs ({d.inputs.length})</p>
        <ul role="list" className="io-list" data-testid="tx-detail-inputs">
          {d.inputs.map((i) => (
            <li key={`${i.previousTxid}:${i.previousVout}`} className="io-row">
              <span className="mono io-ref">{`${i.previousTxid.slice(0, 16)}…:${i.previousVout}`}</span>
              <span>{i.address ?? "—"}</span>
              {i.amount !== null ? <AmountText text={display.full(i.amount)} unitName={unit} glyph={false} /> : <span title="Not a wallet input">—</span>}
            </li>
          ))}
        </ul>
        <p className="io-heading">Outputs ({d.outputs.length})</p>
        <ul role="list" className="io-list" data-testid="tx-detail-outputs">
          {d.outputs.map((o) => (
            <li key={o.vout} className="io-row">
              <span className="mono io-ref">#{o.vout}</span>
              <span>
                {o.address ?? "—"}
                {o.isChange ? " (change)" : o.isMine ? " (own address)" : ""}
              </span>
              <AmountText text={display.full(o.amount)} unitName={unit} glyph={false} />
            </li>
          ))}
        </ul>
      </details>
    </>
  );
}

export function TxDetailSheet({ walletId, txid, network }: { walletId: string; txid: string; network: NetworkInfo }) {
  const detail = useTxDetail(walletId, txid);
  // View contact closed the sheet: its opener (a row) went with the page (D4).
  const viewed = useRef(false);
  return (
    <Sheet
      title="Transaction details"
      width="medium"
      testId="tx-detail"
      onClose={() => ui.set({ detail: null })}
      returnFocus={() => (viewed.current ? contactsFocus() : null)}
    >
      {detail.data ? (
        <Body d={detail.data} network={network} walletId={walletId} onViewContact={() => (viewed.current = true)} />
      ) : detail.isError ? (
        <p role="alert">{errorMessage(detail.error)}</p>
      ) : (
        <LoadingState testId="tx-detail-loading" text="Loading transaction" />
      )}
    </Sheet>
  );
}
