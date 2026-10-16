// Receive (U-03 D11, UX-SPEC §4.8): the receiving address as a QR code and text, copy and new
// address; Request payment stores a request on a fresh address and shows it; the requested
// payments list keeps them (dash-qt's Receive tab, QT-081…085).

import { useId, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage, hasCode } from "../api/backend";
import type { AddressInfo } from "../api/generated/AddressInfo";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ReceiveRequest } from "../api/generated/ReceiveRequest";
import { keys, useBackend, useQrMatrix, useReceiveAddress, useReceiveRequests, useReceiveUsername, useUsernameInRequests } from "../api/queries";
import { useDebounced } from "../app/useDebounced";
import { AmountText, DashButton } from "../components/basics";
import { CopyValue, DetailRow } from "../components/detail";
import { LoadingState } from "../components/history";
import { QrCode } from "../components/qr";
import { Sheet } from "../components/sheet";
import { copyWithToast } from "../components/toast";
import { useDisplay } from "../format/display";
import { shortAddress } from "../format/tx";
import { Page } from "./Routes";

const QR_SIDE = 200;

/** The QR of `text`, or why there is none (dash-qt "Resulting URI too long"). */
function Qr({ text, label, testId }: { text: string | undefined; label: string; testId: string }) {
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

function AddressCard({ walletId }: { walletId: string }) {
  const b = useBackend();
  const current = useReceiveAddress(walletId);
  // "New address" issues one (dash-qt getNewDestination); it is shown until the page closes.
  const [fresh, setFresh] = useState<AddressInfo | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const shown = fresh ?? current.data;
  // The QR carries the wallet's DashPay username as `user=` (DP3-03). The engine reads none
  // while names are updating (a busy identity store during sync), so the card keeps the last
  // name it showed for this wallet rather than dropping it (DP3-02 D25); a failed read is none.
  // With "Include my username in payment requests" off, or not known to be on, it holds none
  // (DP3-03b): the engine's none is then the answer, not a busy store.
  const username = useReceiveUsername(walletId);
  const included = useUsernameInRequests(walletId);
  const held = useRef<{ walletId: string; name: string } | null>(null);
  if (username.data) held.current = { walletId, name: username.data };
  const last = included.data === true && held.current?.walletId === walletId ? held.current.name : null;
  const user =
    included.data === false ? null : username.isError ? last : username.data === undefined ? undefined : (username.data ?? last);
  const uri = useQuery({
    // Its own prefix: Tools ▸ Address book's `paymentUri` key takes a label in the same place.
    queryKey: ["receiveUri", shown?.address ?? "", user ?? null] as const,
    queryFn: () => b.buildPaymentUri(shown!.address, null, null, null, user ?? null),
    enabled: !!shown && user !== undefined,
    staleTime: Infinity,
    // While the username changes the QR keeps the address's previous link, not a loading
    // state; a new address never shows the old one's.
    placeholderData: (prev, prevQuery) => (prevQuery?.queryKey[1] === shown?.address ? prev : undefined),
  });
  const newAddress = async () => {
    setProblem(null);
    try {
      setFresh(await b.nextReceiveAddress(walletId, null));
    } catch (e) {
      setProblem(hasCode(e, "gap_limit") ? "Every address inside the gap limit is in use already. Wait for a payment to one of them." : errorMessage(e));
    }
  };
  return (
    <section className="receive-card card-l1" aria-labelledby="receive-address-title" data-testid="receive-card">
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
          <Qr text={uri.data} label={`QR code of ${shown.address}`} testId="receive-qr" />
          <p className="address-full" data-testid="receive-address">
            {shown.address}
          </p>
          <div className="card-actions">
            <DashButton variant="tintedBlue" data-testid="receive-copy-address" onClick={() => void copyWithToast(shown.address)}>
              Copy address
            </DashButton>
            <DashButton variant="tintedBlue" data-testid="receive-copy-uri" disabled={!uri.data} onClick={() => uri.data && void copyWithToast(uri.data)}>
              Copy link
            </DashButton>
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
    </section>
  );
}

function RequestForm({ walletId, network, onCreated }: { walletId: string; network: NetworkInfo; onCreated: (r: ReceiveRequest) => void }) {
  const b = useBackend();
  const client = useQueryClient();
  const id = useId();
  const [label, setLabel] = useState("");
  const [amountText, setAmountText] = useState("");
  const [message, setMessage] = useState("");
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
  const amountError =
    checked === "" ? null : parsed.data === null ? shown.invalidAmount : parsed.data === 0 ? "The amount must be larger than 0, or empty." : null;
  const ready = !busy && checked === amt && (amt === "" || (typeof parsed.data === "number" && parsed.data > 0));

  const clear = () => {
    setLabel("");
    setAmountText("");
    setMessage("");
    setProblem(null);
  };
  const create = async () => {
    if (!ready) return;
    setBusy(true);
    setProblem(null);
    try {
      const amount = amt === "" ? null : parsed.data || null;
      const r = await b.createReceiveRequest(walletId, amount, label.trim() || null, message.trim() || null);
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
        <label htmlFor={`${id}-amount`}>Amount ({shown.name})</label>
        <input
          id={`${id}-amount`}
          data-testid="request-amount"
          value={amountText}
          inputMode="decimal"
          placeholder="Any amount"
          autoComplete="off"
          aria-invalid={!!amountError}
          aria-describedby={amountError ? `${id}-amount-error` : undefined}
          onChange={(e) => setAmountText(e.target.value)}
        />
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
      {problem && (
        <p className="form-problem" role="alert">
          {problem}
        </p>
      )}
      <div className="card-actions">
        <DashButton variant="tintedGray" onClick={clear}>
          Clear
        </DashButton>
        <DashButton type="submit" variant="filledBlue" data-testid="request-create" disabled={!ready}>
          Create request
        </DashButton>
      </div>
    </form>
  );
}

function RequestSheet({ request, network, onClose }: { request: ReceiveRequest; network: NetworkInfo; onClose: () => void }) {
  const shown = useDisplay(network);
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
                  <td className="num">{r.amount === null ? <span className="text-secondary">(no amount requested)</span> : shown.full(r.amount)}</td>
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
  return (
    <Page route="receive">
      <div className="receive-columns">
        <AddressCard walletId={walletId} />
        <RequestForm walletId={walletId} network={network} onCreated={setShown} />
      </div>
      <RequestList walletId={walletId} network={network} onShow={setShown} />
      {shown && <RequestSheet request={shown} network={network} onClose={() => setShown(null)} />}
    </Page>
  );
}
