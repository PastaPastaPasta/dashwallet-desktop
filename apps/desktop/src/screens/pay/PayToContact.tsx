// Pay ▸ To a contact (DP3-06, DASHPAY §4 `SendViewModel`): the main identity's frequent contacts
// and a searchable picker; once a payee is chosen (here or by the contact pane's Pay), its card,
// the banner saying why it cannot be paid now, and U-03's form without the address: amount, Max,
// subtract fee, note, coin control. Review opens U-03's confirm sheet with the contact, which can
// accept their request and pay in one step; U-03's result follows.

import { useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import type { ContactRecipient } from "../../api/generated/ContactRecipient";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { keys, useBalance, useLockStatus, useWallets } from "../../api/queries";
import { navigate, ui, useUi } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { DashButton, Icon } from "../../components/basics";
import { EmptyState, LoadingState } from "../../components/history";
import { useDisplay } from "../../format/display";
import { SEARCH_DEBOUNCE_MS } from "../../contacts/model";
import { refreshAfter, useContactDetail, useContactsOwner, useHasKeys } from "../../contacts/queries";
import { Outcome as ActionLine } from "../tools/common";
import { Avatar } from "../contacts/Avatar";
import { Row } from "../contacts/ContactList";
import { CoinControl } from "./CoinControl";
import { ConfirmSheet } from "./ConfirmSheet";
import { acceptsToo, blocks, noOwner, payBanner, payeeOf, pickerGroups, resolveText, type Banner } from "./contact";
import { useFrequentContacts, usePayPicker, useResolveLock } from "./contactQueries";
import { AmountFields, draftSpec, FormField, PayFooter, useAmountField, useEstimate, useMax, type Phase } from "./form";
import { confirmedOf, DEFAULT_ADVANCED, type Advanced, type Outcome } from "./model";
import { ResultSheet } from "./ResultSheet";
import { useDraft } from "./useDraft";

export function PayToContact({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const { owner, retry } = useContactsOwner(walletId);
  if (owner.kind === "identity") return <ContactTab key={`${walletId}:${owner.identity}`} walletId={walletId} identity={owner.identity} network={network} />;
  if (owner.kind === "starting") return <LoadingState testId="pay-contact-starting" text="Starting DashPay…" />;
  const none = noOwner(owner);
  return (
    <EmptyState
      testId="pay-contact-none"
      title={none.title}
      message={none.message ?? undefined}
      action={
        <span className="add-contact-actions">
          {none.next === "join" && (
            <DashButton key="join" variant="filledBlue" data-testid="pay-contact-join" onClick={() => navigate("joinDashPay")}>
              Join DashPay
            </DashButton>
          )}
          {none.next === "retry" && (
            <DashButton key="retry" variant="plainBlue" onClick={retry}>
              Try again
            </DashButton>
          )}
          <DashButton key="address" variant="tintedBlue" data-testid="pay-contact-to-address" onClick={() => ui.set({ payTab: "address" })}>
            Pay to an address
          </DashButton>
        </span>
      }
    />
  );
}

/** The identity's tab: the chooser until a payee is picked (D2). */
function ContactTab({ walletId, identity, network }: { walletId: string; identity: string; network: NetworkInfo }) {
  const payee = useUi((s) => s.payContact);
  const pick = (contact: string) => ui.set({ payContact: contact });
  return payee === null ? (
    <Chooser walletId={walletId} identity={identity} onPick={pick} />
  ) : (
    <ContactForm key={payee} walletId={walletId} identity={identity} network={network} contact={payee} />
  );
}

/** The frequent strip (D3) and the picker (D4). */
function Chooser({ walletId, identity, onPick }: { walletId: string; identity: string; onPick: (contact: string) => void }) {
  const id = useId();
  const [search, setSearch] = useState("");
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  const frequent = useFrequentContacts(walletId, identity);
  const list = usePayPicker(walletId, identity, text);
  const groups = pickerGroups(list.data);
  let body;
  if (list.data === undefined && list.isError)
    body = (
      <p className="form-problem" role="alert" data-testid="pay-picker-error">
        Could not read your contacts: {errorMessage(list.error)}{" "}
        <DashButton variant="plainBlue" size="small" onClick={() => void list.refetch()}>
          Try again
        </DashButton>
      </p>
    );
  else if (list.data === undefined) body = <LoadingState testId="pay-picker-loading" text="Loading contacts…" />;
  else if (groups.length === 0)
    body = text ? (
      <EmptyState testId="pay-picker-none" title="No contacts match" message="Search by name or username." />
    ) : (
      <EmptyState
        testId="pay-picker-empty"
        title={list.data.updating ? "Your contacts are being updated" : "No contacts yet"}
        message="Add contacts in Contacts to pay them by name."
        action={
          <DashButton variant="tintedBlue" onClick={() => navigate("contacts")}>
            Open Contacts
          </DashButton>
        }
      />
    );
  else
    body = groups.map((g) => (
      <section key={g.id} className="tx-group card-l1" aria-labelledby={`${id}-${g.id}`} data-testid={`pay-picker-${g.id}`}>
        <h2 className="contacts-group-title" id={`${id}-${g.id}`}>
          {g.title}
        </h2>
        <ul className="tx-group-rows">
          {g.rows.map((r) => (
            <Row
              key={r.contact}
              letters={r.letters}
              name={r.name}
              sub={r.sub}
              trailing={r.reason && <span className="contact-row-note">{r.reason}</span>}
              data-testid="pay-picker-row"
              data-contact={r.contact}
              onClick={() => onPick(r.contact)}
            />
          ))}
        </ul>
      </section>
    ));
  const strip = text === "" ? (frequent.data ?? []) : [];
  return (
    <div className="pay-form pay-contact-chooser" data-testid="pay-contact">
      {strip.length > 0 && (
        <section className="pay-frequent" aria-labelledby={`${id}-frequent`} data-testid="pay-frequent">
          <h2 className="contacts-group-title" id={`${id}-frequent`}>
            Frequent contacts
          </h2>
          <ul className="pay-frequent-chips">
            {strip.map((c) => {
              const p = payeeOf(c);
              return (
                <li key={c.contact}>
                  <button type="button" className="pay-frequent-chip" data-testid="pay-frequent-chip" data-contact={c.contact} onClick={() => onPick(c.contact)}>
                    <Avatar letters={p.letters} size="pane" />
                    <span className="pay-frequent-name">{p.name}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      )}
      <label className="search-field" data-testid="pay-contact-search">
        <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
        <input
          type="search"
          value={search}
          placeholder="Search for a contact"
          aria-label="Search for a contact"
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape" && search !== "") {
              e.preventDefault();
              setSearch("");
            }
          }}
        />
      </label>
      {list.data?.updating && groups.length > 0 && (
        <p className="contacts-updating" role="status">
          Your contacts are being updated.
        </p>
      )}
      {body}
      <p className="placeholder-note">
        Paying someone who is not a contact?{" "}
        <DashButton variant="plainBlue" size="small" data-testid="pay-contact-to-address" onClick={() => ui.set({ payTab: "address" })}>
          Pay to an address
        </DashButton>
      </p>
    </div>
  );
}

/** The chosen payee: its card, the banner (D6), the form, the sheets. */
function ContactForm({ walletId, identity, network, contact }: { walletId: string; identity: string; network: NetworkInfo; contact: string }) {
  const client = useQueryClient();
  const shown = useDisplay(network);
  const lockState = useLockStatus().data?.state;
  const locked = lockState === "locked";
  const draft = useDraft(walletId, locked);
  const balance = useBalance(walletId);
  const watchOnly = useWallets().data?.find((w) => w.walletId === walletId)?.watchOnly;
  const detail = useContactDetail(walletId, identity, contact);
  const hasKeys = useHasKeys(walletId, identity);
  // The accept went through in this form (D11): the next Review pays without it.
  const [accepted, setAccepted] = useState(false);
  const banner = payBanner({
    detail: detail.data,
    readError: detail.data === undefined && detail.isError ? errorMessage(detail.error) : null,
    watchOnly,
    lock: lockState,
    hasKeys,
    accepted,
  });
  const held = blocks(banner);
  // What Check again found (D7), kept once the lock it resolved (and the banner) went.
  const resolve = useResolveLock(walletId, identity, contact);
  const found = resolve.isError ? { text: errorMessage(resolve.error), resolved: false } : resolve.data ? resolveText(resolve.data) : null;
  const payee = payeeOf(detail.data?.summary ?? { contact, alias: null, displayName: null, username: null });

  const amount = useAmountField(shown, "");
  const subtractFeeByDefault = () => ui.get().prefs.subtractFeeByDefault;
  const [subtractFee, setSubtractFee] = useState(subtractFeeByDefault);
  const [note, setNote] = useState("");
  const [advanced, setAdvanced] = useState<Advanced>(DEFAULT_ADVANCED);
  const [phase, setPhase] = useState<Phase>({ kind: "editing" });
  const reviewButton = useRef<HTMLButtonElement>(null);
  const amountField = useRef<HTMLInputElement>(null);
  const backToForm = () => (reviewButton.current?.disabled === false ? reviewButton.current : amountField.current);

  const { duffs } = amount;
  const recipient: ContactRecipient | null =
    typeof duffs === "number" && duffs > 0 ? { identity, contact, amount: duffs, subtractFee, note: note.trim() || null } : null;
  const spec = draftSpec(recipient, advanced);
  const { estimate, general, fieldError, ready: estimated } = useEstimate(walletId, draft, spec, advanced, locked || held, shown);
  const ready = estimated && amount.settled;
  const max = useMax(walletId, advanced, shown, (text) => {
    amount.setText(text);
    setSubtractFee(true);
  });

  const review = () => {
    if (ready && estimate.data && spec) setPhase({ kind: "confirm", confirmed: confirmedOf(spec.recipient, estimate.data, spec.change), spec });
  };
  const reset = () => {
    amount.setText("");
    setSubtractFee(subtractFeeByDefault());
    setNote("");
    setAdvanced(DEFAULT_ADVANCED);
    void draft.close();
    void client.invalidateQueries({ queryKey: keys.coinsAll(walletId) });
  };
  const finish = (outcome: Outcome, show: boolean) => {
    // As To an address: a payment that went, or may have, ends the form; the payee stays.
    if (outcome.kind !== "failed") reset();
    setPhase({ kind: "editing" });
    if (show && outcome.kind === "sent") navigate("activity", { detail: outcome.txid });
  };

  return (
    <>
      <form
        className="pay-form"
        data-testid="pay-contact-form"
        aria-label={`Pay ${payee.name}`}
        onSubmit={(e) => {
          e.preventDefault();
          review();
        }}
      >
        <section className="pay-card card-l1 pay-payee-card" data-testid="pay-payee">
          <span className="pay-payee">
            <Avatar letters={payee.letters} size="pane" />
            <span className="pay-payee-names">
              <span className="pay-payee-name" data-testid="pay-payee-name">
                {payee.name}
              </span>
              {payee.sub && <span className="contact-row-username">{payee.sub}</span>}
            </span>
          </span>
          <DashButton variant="tintedBlue" size="small" data-testid="pay-payee-change" onClick={() => ui.set({ payContact: null })}>
            Change
          </DashButton>
        </section>
        {banner && <PayBanner banner={banner} contact={contact} checking={resolve.isPending} onCheck={() => resolve.mutate()} retry={() => void detail.refetch()} />}
        <ActionLine ok={found?.resolved ?? true} text={found?.text ?? null} testId="pay-lock-resolve" />
        {!held && (
          <>
            <section className="pay-card card-l1">
              <AmountFields
                shown={shown}
                amount={amount}
                max={max}
                error={fieldError("amount")}
                available={balance.data?.confirmed}
                subtractFee={subtractFee}
                onSubtractFee={setSubtractFee}
                inputRef={amountField}
              />
              <FormField label="Note" hint="Kept with the transaction in this wallet; not sent to the contact.">
                {(p) => <input {...p} data-testid="pay-note" value={note} autoComplete="off" onChange={(e) => setNote(e.target.value)} />}
              </FormField>
            </section>
            <CoinControl walletId={walletId} network={network} advanced={advanced} onChange={setAdvanced} payAmount={typeof duffs === "number" ? duffs : null} />
            <PayFooter problem={general} fee={estimate.data && spec ? estimate.data.fee : null} shown={shown} ready={ready} reviewRef={reviewButton} />
          </>
        )}
      </form>
      {phase.kind === "confirm" && (
        <ConfirmSheet
          confirmed={phase.confirmed}
          spec={phase.spec}
          coins={advanced}
          walletId={walletId}
          network={network}
          draft={draft}
          returnFocus={backToForm}
          onCancel={() => setPhase({ kind: "editing" })}
          onOutcome={(outcome) => setPhase({ kind: "result", outcome })}
          contact={{
            identity,
            payee,
            acceptsToo: acceptsToo(detail.data, accepted),
            onAccepted: () => {
              setAccepted(true);
              refreshAfter(client, walletId, identity, ["contacts"]);
            },
          }}
        />
      )}
      {phase.kind === "result" && (
        <ResultSheet
          outcome={phase.outcome}
          network={network}
          draft={draft}
          returnFocus={backToForm}
          onOutcome={(outcome) => setPhase({ kind: "result", outcome })}
          onDone={(show) => finish(phase.outcome, show)}
        />
      )}
    </>
  );
}

/** Why the payee cannot be paid now, and what to do (D6, D7). */
function PayBanner({ banner, contact, checking, onCheck, retry }: { banner: Banner; contact: string; checking: boolean; onCheck: () => void; retry: () => void }) {
  if (banner.kind === "loading") return <LoadingState testId="pay-contact-loading" text="Reading the contact…" />;
  let action = null;
  switch (banner.kind) {
    case "readFailed":
      action = (
        <DashButton variant="tintedBlue" size="small" onClick={retry}>
          Try again
        </DashButton>
      );
      break;
    case "enable":
      action = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-banner-contacts" onClick={() => navigate("contacts", { contact })}>
          Open in Contacts
        </DashButton>
      );
      break;
    case "paymentLock":
      action = (
        <span className="add-contact-actions">
          <DashButton key="show" variant="tintedGray" size="small" data-testid="pay-banner-show" onClick={() => navigate("activity", { detail: banner.txid })}>
            Show transaction
          </DashButton>
          <DashButton key="check" variant="tintedBlue" size="small" data-testid="pay-banner-check" disabled={checking} onClick={onCheck}>
            {checking ? "Checking…" : "Check again"}
          </DashButton>
        </span>
      );
      break;
    case "mixingOnly":
      action = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-banner-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
          Unlock
        </DashButton>
      );
      break;
  }
  return (
    <div className={`contacts-hint card-l1 pay-banner is-${banner.kind}`} role="status" data-testid="pay-banner" data-kind={banner.kind}>
      <span data-testid="pay-banner-text">{banner.text}</span>
      {action}
    </div>
  );
}
