// Receive (U-03 D11, UX-SPEC §4.8): the receiving address as a QR code and text, copy and new
// address; Request payment stores a request on a fresh address and shows it; the requested
// payments list keeps them (dash-qt's Receive tab, QT-081…085). The Dash, Platform and Shielded
// tabs above both columns switch the address; payment requests are for Dash addresses only, so on
// the other tabs the request column says what the address is for instead.

import { useEffect, useId, useRef, useState } from "react";
import { useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { errorMessage, hasCode } from "../api/backend";
import type { AddressInfo } from "../api/generated/AddressInfo";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ReceiveRequest } from "../api/generated/ReceiveRequest";
import { keys, useBackend, usePlatformAddresses, usePlatformReceiveAddress, useQrMatrix, useShieldedStatus, useReceiveAddress, useReceiveRequests, useReceiveUsername, useUsernameAllowed } from "../api/queries";
import { withoutUsername } from "../api/username";
import { useDebounced } from "../app/useDebounced";
import { AmountText, DashButton } from "../components/basics";
import { CopyValue, DetailRow } from "../components/detail";
import { LoadingState } from "../components/history";
import { QrCode } from "../components/qr";
import { Sheet } from "../components/sheet";
import { CopyDashButton } from "../components/copy";
import { useUi } from "../app/ui";
import { FiatSecondary, FiatToggle, useFiatEntry } from "../fiat/entry";
import { FiatLine, useFiat } from "../fiat/rates";
import { useDisplay } from "../format/display";
import { useHeldAddress } from "../payments/hold";
import { useLookupPending, useReceived } from "../payments/received";
import { receivedText } from "./notifications/Received";
import { shortAddress } from "../format/tx";
import { shieldedSyncing, SHIELDED_SYNCING_TEXT } from "../shielded/syncing";
import { Page } from "./Routes";
import { SHIELDED_COPY } from "../shielded/flow";
import { useShieldedReceiveContent } from "./ShieldedSheet";

const QR_SIDE = 200;

/** The QR of `text`, or why there is none (dash-qt "Resulting URI too long"). */
export function Qr({ text, label, testId }: { text: string | undefined; label: string; testId: string }) {
  const qr = useQrMatrix(text);
  if (qr.data) return <QrCode matrix={qr.data} size={QR_SIDE} label={label} testId={testId} />;
  if (qr.isError)
    return (
      <p className="qr-problem" role="alert">
        {hasCode(qr.error, "uri.too_long_for_qr")
          ? "Resulting URI too long, try to reduce the text for label / message."
          : `Could not draw the QR code: ${errorMessage(qr.error)}`}
      </p>
    );
  return <div className="qr-placeholder" style={{ width: QR_SIDE, height: QR_SIDE }} aria-hidden="true" />;
}

/** The name the QR carries: none until the setting has read on (`undefined`: wait). */
function qrUsername(included: boolean | undefined, username: UseQueryResult<string | null>, last: string | null): string | null | undefined {
  if (included === undefined) return undefined;
  if (!included) return null;
  if (username.isError) return last;
  if (username.data === undefined) return undefined;
  return username.data ?? last;
}

function useDashReceiveContent(walletId: string, network: NetworkInfo) {
  const b = useBackend();
  const current = useReceiveAddress(walletId);
  // "New address" issues one (dash-qt getNewDestination); it is shown until the page closes.
  const [fresh, setFresh] = useState<AddressInfo | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // A payment to the address on show is said first; the engine's next address follows (DEC-1058 #6).
  const { shown, paid } = useHeldAddress(current.data, current.isSuccess && !current.isFetching, fresh, useReceived(walletId), useLookupPending(walletId));
  const display = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  // The QR carries the wallet's DashPay username as `user=` (DP3-03). The engine reads none
  // while names are updating (a busy identity store during sync), so the card keeps the last
  // name it showed for this wallet rather than dropping it (DP3-02 D25); a failed read is none.
  // "Include my username in payment requests" decides first (DP3-03b D22): until it has read on,
  // no name is used, held or read; off, the engine's none is the answer, not a busy store.
  const username = useReceiveUsername(walletId);
  const included = useUsernameAllowed(walletId);
  const held = useRef<{ walletId: string; name: string } | null>(null);
  if (username.data) held.current = { walletId, name: username.data };
  const last = held.current?.walletId === walletId ? held.current.name : null;
  const user = qrUsername(included, username, last);
  const uri = useQuery({
    // Its own prefix: Tools ▸ Address book's `paymentUri` key takes a label in the same place.
    queryKey: ["receiveUri", shown?.address ?? "", user ?? null] as const,
    queryFn: () => b.buildPaymentUri(shown!.address, null, null, null, user ?? null),
    enabled: !!shown && user !== undefined,
    staleTime: Infinity,
    // While the username changes the QR keeps the address's previous link, not a loading
    // state; a new address never shows the old one's, and a link without a name never shows
    // one with it (DP3-03b D18).
    placeholderData: (prev, prevQuery) =>
      prevQuery?.queryKey[1] === shown?.address && (typeof user === "string" || prevQuery?.queryKey[2] === null) ? prev : undefined,
  });
  const newAddress = async () => {
    setProblem(null);
    try {
      setFresh(await b.nextReceiveAddress(walletId, null));
    } catch (e) {
      setProblem(hasCode(e, "gap_limit") ? "Every address inside the gap limit is in use already. Wait for a payment to one of them." : errorMessage(e));
    }
  };
  return {
    before: <>
      <h2 id="receive-address-title" className="card-title">
        Your Dash address
      </h2>
      {current.isError && !fresh ? (
        <p className="form-problem" role="alert">
          Could not load the receiving address: {errorMessage(current.error)}
        </p>
      ) : !shown ? (
        <LoadingState testId="receive-loading" text="Loading address" />
      ) : (
        <>
          {paid.length > 0 && (
            <p className="receive-received" data-testid="receive-received">
              <span aria-hidden="true">✓</span> {receivedText(paid.reduce((n, p) => n + p.duffs, 0), display, discreet)}
            </p>
          )}
          <div className={paid.length > 0 ? "receive-qr is-used" : "receive-qr"}>
            <Qr text={uri.data} label={`QR code of ${shown.address}`} testId="receive-qr" />
          </div>
        </>
      )}
    </>,
    after: <>
      {shown && (!current.isError || fresh) && (
        <>
          <p className="address-full" data-testid="receive-address">
            {shown.address}
          </p>
          <div className="card-actions">
            <CopyDashButton variant="tintedBlue" data-testid="receive-copy-address" text={shown.address}>
              Copy address
            </CopyDashButton>
            <CopyDashButton variant="tintedBlue" data-testid="receive-copy-uri" disabled={!uri.data} why="The payment link is not ready yet." text={uri.data ?? null}>
              Copy link
            </CopyDashButton>
            <DashButton variant="plainBlue" data-testid="receive-new-address" onClick={() => void newAddress()}>
              New address
            </DashButton>
          </div>
        </>
      )}
      {problem && (
        <p className="form-problem" role="alert">
          {problem}
        </p>
      )}
    </>,
  };
}

export const PLATFORM_RECEIVE_COPY = {
  text: "This address holds Dash on Dash Platform. Anyone with a Platform address can pay it.",
  warning: "Most Dash wallets and exchanges cannot pay this address.",
  loading: "Loading Platform address",
  copy: "Copy address",
  failed: "Could not load the Platform address:",
} as const;

/** The selected Platform address uses a bare QR; Dash payment URIs cannot carry it. */
function usePlatformReceiveContent(walletId: string, enabled: boolean) {
  const b = useBackend();
  const client = useQueryClient();
  const address = usePlatformReceiveAddress(walletId, enabled);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    b.platformAddressesSync(walletId)
      .then(() => live && void client.invalidateQueries({ queryKey: ["dashpay"] }))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [b, client, walletId, enabled]);
  const C = PLATFORM_RECEIVE_COPY;
  return {
    before: <>
      <h2 id="receive-address-title" className="card-title">Your Platform address</h2>
      {address.isError ? (
        <p className="form-problem" role="alert" data-testid="platform-receive-error">
          {C.failed} {errorMessage(address.error)}
        </p>
      ) : !address.data ? (
        <LoadingState testId="platform-receive-loading" text={C.loading} />
      ) : (
        <>
          <div className="receive-qr">
            <Qr text={address.data.address} label={`QR code of ${address.data.address}`} testId="platform-receive-qr" />
          </div>
        </>
      )}
    </>,
    after: <>
      {!address.isError && address.data && (
        <>
          <p className="address-full" data-testid="platform-receive-address">
            {address.data.address}
          </p>
          <div className="card-actions">
            <CopyDashButton variant="tintedBlue" data-testid="platform-receive-copy" text={address.data.address}>
              {C.copy}
            </CopyDashButton>
          </div>
        </>
      )}
    </>,
  };
}

const ABOUT_COPY = { title: "About this address", dashOnly: "Payment requests are for Dash addresses only." } as const;

/** The request column on Platform and Shielded: no payment URI carries those addresses (`build_payment_uri`
 * is Core's), so it says what the address is for and who can pay it. */
function RequestUnavailable({ pool }: { pool: "platform" | "shielded" }) {
  return (
    <section className="receive-card card-l1 request-unavailable" aria-labelledby="request-unavailable-title" data-testid="receive-request-unavailable" data-pool={pool}>
      <h2 id="request-unavailable-title" className="card-title">
        {ABOUT_COPY.title}
      </h2>
      <p>{pool === "platform" ? PLATFORM_RECEIVE_COPY.text : SHIELDED_COPY.receive.text}</p>
      {pool === "platform" && (
        <p className="field-hint" data-testid="platform-receive-warning">
          {PLATFORM_RECEIVE_COPY.warning}
        </p>
      )}
      <p className="placeholder-note">{ABOUT_COPY.dashOnly}</p>
    </section>
  );
}

/** `inactive`: another tab is shown; the form keeps what was typed and its place, out of sight and reach. */
function RequestForm({ walletId, network, onCreated, inactive }: { walletId: string; network: NetworkInfo; onCreated: (r: ReceiveRequest) => void; inactive: boolean }) {
  const b = useBackend();
  const client = useQueryClient();
  const id = useId();
  const [label, setLabel] = useState("");
  const [amountText, setAmountText] = useState("");
  const [message, setMessage] = useState("");
  const allowed = useUsernameAllowed(walletId);
  const [includeUsername, setIncludeUsername] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const amt = amountText.trim();
  const checked = useDebounced(amt, 300);
  const shown = useDisplay(network);
  const parsed = useQuery({
    queryKey: ["parseAmount", shown.unit, checked],
    queryFn: () => b.parseAmount(checked, shown.unit),
    enabled: checked !== "",
    staleTime: Infinity,
  });
  // The amount may be typed in fiat (MP-01): the request keeps the duffs it converts to.
  const fiat = useFiatEntry(shown, { text: amountText, setText: setAmountText, duffs: checked !== "" ? parsed.data : undefined });
  const amountError =
    fiat.error ??
    (checked === "" ? null : parsed.data === null ? shown.invalidAmount : parsed.data === 0 ? "The amount must be larger than 0, or empty." : null);
  // Fiat that is not an amount, or too large, leaves the DASH text empty: never an "any amount"
  // request from a fiat field holding text.
  const fiatLeftEmpty = fiat.active && fiat.value.trim() !== "" && amt === "";
  const ready = allowed !== undefined && !busy && !fiat.error && !fiatLeftEmpty && checked === amt && (amt === "" || (typeof parsed.data === "number" && parsed.data > 0));

  const clear = () => {
    setLabel("");
    setAmountText("");
    setMessage("");
    setIncludeUsername(null);
    setProblem(null);
  };
  const create = async () => {
    if (!ready) return;
    setBusy(true);
    setProblem(null);
    try {
      const amount = amt === "" ? null : parsed.data || null;
      const r = await b.createReceiveRequest(walletId, amount, label.trim() || null, message.trim() || null, includeUsername ?? allowed === true);
      void client.invalidateQueries({ queryKey: keys.receiveAll(walletId) });
      clear();
      onCreated(r);
    } catch (e) {
      setProblem(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      inert={inactive}
      className="receive-card card-l1 request-form"
      aria-labelledby={`${id}-title`}
      data-testid="request-form"
      onSubmit={(e) => {
        e.preventDefault();
        void create();
      }}
    >
      <h2 id={`${id}-title`} className="card-title">
        Request payment
      </h2>
      <p className="placeholder-note">Use this form to request payments. All fields are optional.</p>
      <div className="form-field">
        <label htmlFor={`${id}-label`}>Label</label>
        <input id={`${id}-label`} data-testid="request-label" value={label} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />
      </div>
      <div className="form-field">
        <label htmlFor={`${id}-amount`}>Amount ({fiat.unitName})</label>
        <span className="amount-entry">
          <input
            id={`${id}-amount`}
            data-testid="request-amount"
            value={fiat.value}
            inputMode="decimal"
            placeholder="Any amount"
            autoComplete="off"
            aria-invalid={!!amountError}
            aria-describedby={amountError ? `${id}-amount-error` : undefined}
            onChange={(e) => fiat.onChange(e.target.value)}
          />
          <FiatToggle entry={fiat} shown={shown} testId="request-amount-toggle" />
        </span>
        <FiatSecondary entry={fiat} duffs={checked !== "" ? parsed.data : undefined} testId="request-amount-other" />
        {amountError && (
          <span id={`${id}-amount-error`} className="field-error">
            {amountError}
          </span>
        )}
      </div>
      <div className="form-field">
        <label htmlFor={`${id}-message`}>Message</label>
        <input id={`${id}-message`} data-testid="request-message" value={message} autoComplete="off" onChange={(e) => setMessage(e.target.value)} />
      </div>
      <label className="checkbox-row">
        <input type="checkbox" data-testid="request-include-username" disabled={allowed === undefined} checked={includeUsername ?? allowed === true} onChange={(e) => setIncludeUsername(e.target.checked)} />
        Include my username in this request
      </label>
      {problem && (
        <p className="form-problem" role="alert">
          {problem}
        </p>
      )}
      <div className="card-actions">
        <DashButton variant="tintedGray" onClick={clear}>
          Clear
        </DashButton>
        <DashButton type="submit" variant="filledBlue" data-testid="request-create" disabled={!ready} why={allowed === undefined ? "Loading username preference." : "Enter an amount first."}>
          Create request
        </DashButton>
      </div>
    </form>
  );
}

function RequestSheet({ walletId, request: read, network, onClose }: { walletId: string; request: ReceiveRequest; network: NetworkInfo; onClose: () => void }) {
  const shown = useDisplay(network);
  // A request's explicit choice overrides the wallet default; legacy requests follow it.
  const allowed = useUsernameAllowed(walletId);
  const request = (read.includeUsername ?? allowed) === true ? read : withoutUsername(read);
  const title = `Request payment to ${request.label || shortAddress(request.address)}`;
  return (
    <Sheet title={title} width="medium" testId="request-sheet" onClose={onClose}>
      <div className="request-qr">
        <Qr text={request.uri} label={`QR code of the payment request to ${request.address}`} testId="request-qr" />
      </div>
      <dl className="detail-list card-l1" data-testid="request-facts">
        <DetailRow label="Link" testId="request-uri">
          <CopyValue value={request.uri} label="payment link" />
        </DetailRow>
        <DetailRow label="Address" testId="request-address">
          <CopyValue value={request.address} label="address" />
        </DetailRow>
        {request.amount !== null && (
          <DetailRow label="Amount" testId="request-amount-value">
            <AmountText text={shown.full(request.amount)} unitName={shown.name} glyph={false} />
          </DetailRow>
        )}
        {request.label && <DetailRow label="Label">{request.label}</DetailRow>}
        {request.message && <DetailRow label="Message">{request.message}</DetailRow>}
        {request.username && (
          <DetailRow label="DashPay username" testId="request-username">
            @{request.username}
          </DetailRow>
        )}
      </dl>
    </Sheet>
  );
}

function RequestList({ walletId, network, onShow }: { walletId: string; network: NetworkInfo; onShow: (r: ReceiveRequest) => void }) {
  const b = useBackend();
  const client = useQueryClient();
  const requests = useReceiveRequests(walletId);
  const shown = useDisplay(network);
  const fiat = useFiat();
  const discreet = useUi((s) => s.discreet);
  const [problem, setProblem] = useState<string | null>(null);
  const remove = async (r: ReceiveRequest) => {
    setProblem(null);
    try {
      await b.deleteReceiveRequest(walletId, r.id);
      void client.invalidateQueries({ queryKey: keys.receiveRequests(walletId) });
    } catch (e) {
      setProblem(errorMessage(e));
    }
  };
  const rows = requests.data ?? [];
  return (
    <section className="requests" aria-labelledby="requests-title" data-testid="requests">
      <h2 id="requests-title" className="history-title">
        Requested payments
      </h2>
      {requests.isError ? (
        <p className="form-problem">Could not load the requests: {errorMessage(requests.error)}</p>
      ) : rows.length === 0 ? (
        <p className="placeholder-note" data-testid="requests-empty">
          {requests.isPending ? "Loading requests…" : "No payment requests yet."}
        </p>
      ) : (
        <div className="card-l1 table-card">
          <table className="data-table" data-testid="requests-table">
            <thead>
              <tr>
                <th scope="col">Date</th>
                <th scope="col">Label</th>
                <th scope="col">Message</th>
                <th scope="col" className="num">
                  Requested ({shown.name})
                </th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id} data-testid={`request-row-${r.id}`}>
                  <td>{new Date(r.createdAt * 1000).toLocaleDateString(b.locale, { year: "numeric", month: "short", day: "numeric" })}</td>
                  <td>{r.label || <span className="text-secondary">(no label)</span>}</td>
                  <td>{r.message || <span className="text-secondary">(no message)</span>}</td>
                  <td className="num">
                    {r.amount === null ? (
                      <span className="text-secondary">(no amount requested)</span>
                    ) : (
                      <>
                        {shown.full(r.amount)}
                        <FiatLine fiat={fiat} duffs={r.amount} hidden={discreet} className="request-fiat" testId={`request-fiat-${r.id}`} />
                      </>
                    )}
                  </td>
                  <td className="row-actions">
                    <DashButton variant="plainBlue" size="small" data-testid={`request-show-${r.id}`} onClick={() => onShow(r)}>
                      Show
                    </DashButton>
                    <DashButton variant="plainRed" size="small" data-testid={`request-remove-${r.id}`} aria-label={`Remove the request ${r.label || shortAddress(r.address)}`} onClick={() => void remove(r)}>
                      Remove
                    </DashButton>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {problem && (
        <p className="form-problem" role="alert">
          {problem}
        </p>
      )}
    </section>
  );
}

export function ReceivePage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const [shown, setShown] = useState<ReceiveRequest | null>(null);
  const [pool, setPool] = useState<"dash" | "platform" | "shielded">("dash");
  const platform = usePlatformAddresses(walletId);
  const shielded = useShieldedStatus(walletId);
  const selector = (
    <div className="segmented receive-selector" role="group" aria-label="Receive in">
      {(["dash", "platform", "shielded"] as const).map((p) => (
        <button
          type="button"
          key={p}
          className="segment"
          aria-pressed={pool === p}
          data-testid={p === "dash" ? "receive-select-dash" : p === "platform" ? "platform-receive-toggle" : "shielded-receive-open"}
          disabled={p === "platform" ? !platform.data : p === "shielded" ? !shielded.data?.active : false}
          onClick={() => setPool(p)}
        >
          {p === "dash" ? "Dash" : p === "platform" ? "Platform" : "Shielded"}
        </button>
      ))}
    </div>
  );
  const dashContent = useDashReceiveContent(walletId, network);
  const platformContent = usePlatformReceiveContent(walletId, pool === "platform");
  const syncing = shieldedSyncing(shielded.data);
  const shieldedContent = useShieldedReceiveContent(walletId, network, pool === "shielded" && !syncing);
  const content = pool === "dash" ? dashContent : pool === "platform" ? platformContent : {
    before: <>
      <h2 id="receive-address-title" className="card-title">Your Shielded address</h2>
      {syncing ? <p className="field-hint" data-testid="shielded-receive-why">{SHIELDED_SYNCING_TEXT}</p> : shieldedContent.before}
    </>,
    after: syncing ? null : shieldedContent.after,
  };
  return (
    <Page route="receive">
      {selector}
      <div className="receive-columns">
        <section className="receive-card card-l1" aria-labelledby="receive-address-title" data-testid={pool === "dash" ? "receive-card" : pool === "platform" ? "platform-receive" : "shielded-receive-section"}>
          {content.before}
          {content.after}
        </section>
        <div className="receive-request" data-testid="receive-request-column">
          <RequestForm walletId={walletId} network={network} onCreated={setShown} inactive={pool !== "dash"} />
          {pool !== "dash" && <RequestUnavailable pool={pool} />}
        </div>
      </div>
      <RequestList walletId={walletId} network={network} onShow={setShown} />
      {shown && <RequestSheet walletId={walletId} request={shown} network={network} onClose={() => setShown(null)} />}
    </Page>
  );
}
