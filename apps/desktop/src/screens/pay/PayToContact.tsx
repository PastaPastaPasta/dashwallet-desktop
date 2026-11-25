// Pay ▸ To a contact (DP3-06, DASHPAY §4 `SendViewModel`): the main identity's frequent contacts
// and a searchable picker; once a payee is chosen (here or by the contact pane's Pay), its card,
// the banner saying why it cannot be paid now, and U-03's form without the address: amount, Max,
// subtract fee, note, coin control. Review opens U-03's confirm sheet with the contact, which can
// accept their request and pay in one step; U-03's result follows.

import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import type { ContactRecipient } from "../../api/generated/ContactRecipient";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { UserHit } from "../../api/generated/UserHit";
import { keys, useLockStatus, usePlatformActivity, usePoolBalances, useShieldedStatus, useWallets } from "../../api/queries";
import { navigate, ui, useUi } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { DashButton, Icon, VisuallyHidden } from "../../components/basics";
import { EMPTY } from "../../components/emptyStates";
import { EmptyState, focusWhenIdle, LoadingState } from "../../components/history";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { NETWORK_DEBOUNCE_MS, networkPrefix, SEARCH_DEBOUNCE_MS, shortId } from "../../contacts/model";
import { refreshAfter, useContactDetail, useContactsOwner, useHasKeys, useNetworkSearch } from "../../contacts/queries";
import { Outcome as ActionLine } from "../tools/common";
import { Avatar } from "../contacts/Avatar";
import { NetworkResults, Row } from "../contacts/ContactList";
import { CoinControl } from "./CoinControl";
import { FoundUser } from "./FoundUser";
import { FrequentStrip } from "./FrequentStrip";
import { ConfirmSheet } from "./ConfirmSheet";
import { BalanceAmount, BalancePaySheet, FromStep, useBalancePay } from "./PayFromBalance";
import { chosenReason, fromRows, liveSources, type PaySource } from "./fromBalance";
import { shieldedSpendsOff } from "../../shielded/spends";
import { acceptsToo, blocks, noOwner, payBanner, payeeOf, pickerGroups, pickerHits, pickerListing, pickerNetworkState, pickerView, resolveText, searchAnnouncement, type Banner, type Payee } from "./contact";
import { useFrequentContacts, usePayPicker, useResolveLock } from "./contactQueries";
import { AmountFields, draftSpec, FormField, PayFooter, useAvailable, useEstimate, useMax, payFormKey, usePayAmount, usePaySheets } from "./form";
import { confirmedOf, DEFAULT_ADVANCED, type Advanced } from "./model";
import { ResultSheet, SendingSheet } from "./ResultSheet";
import { useDraft } from "./useDraft";

export function PayToContact({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const t = useT();
  const { owner, retry } = useContactsOwner(walletId);
  if (owner.kind === "identity") return <ContactTab key={`${walletId}:${owner.identity}`} walletId={walletId} identity={owner.identity} network={network} />;
  if (owner.kind === "starting") return <LoadingState testId="pay-contact-starting" text={t("pay.contact.starting")} />;
  const none = noOwner(owner);
  // Without a username the tab is one line: Pay opened on To an address already.
  if (none.next === "join")
    return (
      <p className="placeholder-note" data-testid="pay-contact-none">
        {t("pay.contact.joinLine")}{" "}
        <DashButton variant="plainBlue" size="small" data-testid="pay-contact-join" onClick={() => navigate("joinDashPay")}>
          {t("pay.contact.join")}
        </DashButton>
      </p>
    );
  return (
    <EmptyState
      testId="pay-contact-none"
      art={EMPTY.join.art}
      title={none.title}
      message={none.message ?? undefined}
      action={
        <span className="add-contact-actions">
          {none.next === "retry" && (
            <DashButton key="retry" variant="plainBlue" onClick={retry}>
              {t("pay.contact.retry")}
            </DashButton>
          )}
          <DashButton key="address" variant="tintedBlue" data-testid="pay-contact-to-address" onClick={() => ui.set({ payTab: "address" })}>
            {t("pay.contact.toAddress")}
          </DashButton>
        </span>
      }
    />
  );
}

/** The identity's tab: the chooser until a payee is picked (D2). */
function ContactTab({ walletId, identity, network }: { walletId: string; identity: string; network: NetworkInfo }) {
  const picked = useUi((s) => s.payContact);
  const payee = picked?.walletId === walletId ? picked.contact : null;
  // How the chooser showed the pick: the card's names until the payee's detail is read.
  const [shownAs, setShownAs] = useState<Payee | null>(null);
  const pick = (p: Payee) => {
    setShownAs(p);
    ui.set({ payContact: { walletId, contact: p.contact } });
  };
  return payee === null ? (
    <Chooser walletId={walletId} identity={identity} onPick={pick} />
  ) : (
    <ContactForm
      key={payee}
      walletId={walletId}
      identity={identity}
      network={network}
      contact={payee}
      shownAs={shownAs?.contact === payee ? shownAs : null}
    />
  );
}

/** The frequent strip (D3), the picker (D4), and under it Add contact's username search: a user
 * found there gets their own card. */
function Chooser({ walletId, identity, onPick }: { walletId: string; identity: string; onPick: (payee: Payee) => void }) {
  const t = useT();
  const id = useId();
  const [search, setSearch] = useState("");
  const [found, setFound] = useState<UserHit | null>(null);
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  // As Contacts: the username search waits longer, and clearing the field hides it at once.
  const typed = networkPrefix(search);
  const debounced = useDebounced(typed, NETWORK_DEBOUNCE_MS);
  const prefix = typed === null ? null : debounced;
  // The prefix the shown text searches for: until the search catches up with it, it is running.
  const wanted = networkPrefix(text);
  const frequent = useFrequentContacts(walletId, identity);
  const list = usePayPicker(walletId, identity, text);
  const all = usePayPicker(walletId, identity, "");
  const net = useNetworkSearch(walletId, identity, prefix);
  const groups = pickerGroups(list.data);
  // Everyone the search found that the picker does not list for the text is listed under it (a
  // request sent, an ignored sender, oneself, a contact the filter missed), not only strangers.
  const listing = pickerListing({ data: list.data, placeholder: list.isPlaceholderData, failed: list.isError });
  const listReady = listing !== null;
  const hits = listing ? pickerHits(net.data, listing) : { shown: [], more: false, count: 0 };
  const networkState = pickerNetworkState({ listReady, caughtUp: prefix === wanted, pending: net.isPending, failed: net.isError, fetching: net.isFetching });
  const view = pickerView({
    text,
    groups: groups.length,
    anyContacts: pickerGroups(all.data).length > 0,
    network: wanted !== null,
    networkState,
    hits: hits.shown.length,
  });
  const announce = searchAnnouncement({ network: wanted !== null, networkState, hits: hits.count, none: view.none });
  const toAddress = () => ui.set({ payTab: "address" });
  // Change comes back to the search field.
  const field = useRef<HTMLInputElement>(null);
  const back = useRef(false);
  useEffect(() => {
    if (found || !back.current) return;
    back.current = false;
    field.current?.focus();
  }, [found]);
  if (found)
    return (
      <FoundUser
        walletId={walletId}
        identity={identity}
        hit={found}
        onPay={onPick}
        onBack={() => {
          back.current = true;
          setFound(null);
        }}
      />
    );
  let body;
  if (list.data === undefined && list.isError)
    body = (
      <p className="form-problem" role="alert" data-testid="pay-picker-error">
        {t("pay.contact.pickerError", { error: errorMessage(list.error) })}{" "}
        <DashButton variant="plainBlue" size="small" onClick={() => void list.refetch()}>
          {t("pay.contact.retry")}
        </DashButton>
      </p>
    );
  else if (list.data === undefined) body = <LoadingState testId="pay-picker-loading" text={t("pay.contact.loading")} />;
  else if (view.none === "anyone")
    body = (
      <EmptyState
        testId="pay-picker-none"
        art="search"
        title={t("pay.contact.noOneFound")}
        message={t("pay.contact.noOneMessage", { text })}
        action={
          <DashButton variant="tintedBlue" data-testid="pay-none-to-address" onClick={toAddress}>
            {t("pay.contact.toAddressInstead")}
          </DashButton>
        }
      />
    );
  else if (view.none === "contacts")
    body = (
      <EmptyState
        testId="pay-picker-none"
        art="search"
        title={t("pay.contact.noMatch")}
        message={t("pay.contact.noMatchMessage")}
        action={
          <DashButton
            variant="filledBlue"
            data-testid="pay-picker-clear"
            onClick={() => {
              setSearch("");
              focusWhenIdle(field);
            }}
          >
            {t("pay.contact.clear")}
          </DashButton>
        }
      />
    );
  else if (view.none === "short")
    body = (
      <EmptyState
        testId="pay-picker-empty"
        art="contacts"
        title={list.data.updating ? t("pay.contact.updatingTitle") : t("pay.contact.noContacts")}
        message={t("pay.contact.keepTyping")}
      />
    );
  else if (groups.length === 0 && text === "")
    body = (
      <EmptyState
        testId="pay-picker-empty"
        art="contacts"
        title={list.data.updating ? t("pay.contact.updatingTitle") : t("pay.contact.noContacts")}
        message={t("pay.contact.typeUsername")}
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
              identity={r.contact}
              name={r.name}
              sub={r.sub}
              trailing={r.reason && <span className="contact-row-note">{r.reason}</span>}
              data-testid="pay-picker-row"
              data-contact={r.contact}
              onClick={() => onPick(r)}
            />
          ))}
        </ul>
      </section>
    ));
  const strip = text === "" ? (frequent.data ?? []) : [];
  return (
    <div className="pay-form pay-contact-chooser" data-testid="pay-contact">
      <FrequentStrip contacts={strip} onPick={onPick} testId="pay-frequent" />
      <label className="search-field" data-testid="pay-contact-search">
        <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
        <input
          ref={field}
          type="search"
          value={search}
          placeholder={t("pay.contact.search")}
          aria-label={t("pay.contact.search")}
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
          {t("pay.contact.updating")}
        </p>
      )}
      {body}
      <VisuallyHidden>
        <span role="status" data-testid="pay-search-announce">
          {announce}
        </span>
      </VisuallyHidden>
      {view.network && wanted !== null && (
        <NetworkResults
          state={networkState}
          shown={hits.shown}
          more={hits.more}
          onRetry={() => void net.refetch()}
          onPick={setFound}
          onMore={() => navigate("contacts", { addContact: { walletId, query: wanted, hit: null }, contact: null, contactPrefill: null })}
        />
      )}
      {view.none !== "anyone" && (
        <p className="placeholder-note">
          {t("pay.contact.notOnDashPay")}{" "}
          <DashButton variant="plainBlue" size="small" data-testid="pay-contact-to-address" onClick={toAddress}>
            {t("pay.contact.toAddress")}
          </DashButton>
        </p>
      )}
    </div>
  );
}

/** The chosen payee: its card, the banner (D6), the form, the sheets. */
function ContactForm({
  walletId,
  identity,
  network,
  contact,
  shownAs,
}: {
  walletId: string;
  identity: string;
  network: NetworkInfo;
  contact: string;
  shownAs: Payee | null;
}) {
  const t = useT();
  const client = useQueryClient();
  const shown = useDisplay(network);
  const lockState = useLockStatus().data?.state;
  const locked = lockState === "locked";
  const draft = useDraft(walletId, locked);
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
  const lockHeld = blocks(banner);
  // What Check again found (D7), kept once the lock it resolved (and the banner) went, but not
  // under a new lock: it spoke of another payment.
  const resolve = useResolveLock(walletId, identity, contact);
  const lockTxid = detail.data?.paymentLock?.txid ?? null;
  const [checked, setChecked] = useState<string | null>(null);
  const current = lockTxid === null || lockTxid === checked;
  const check = () => {
    setChecked(lockTxid);
    resolve.mutate();
  };
  const found = !current ? null : resolve.isError ? { text: errorMessage(resolve.error), resolved: false } : resolve.data ? resolveText(resolve.data) : null;
  const summary = detail.data?.summary;
  const payee = summary ? payeeOf(summary) : (shownAs ?? payeeOf({ contact, alias: null, displayName: null, username: null }));

  const amount = usePayAmount(shown, "");
  const subtractFeeByDefault = () => ui.get().prefs.subtractFeeByDefault;
  const [subtractFee, setSubtractFee] = useState(subtractFeeByDefault);
  const [note, setNote] = useState("");
  const [advanced, setAdvanced] = useState<Advanced>(DEFAULT_ADVANCED);
  // As To an address: a payment that went, or may have, ends the form; the payee stays.
  const sheets = usePaySheets(payFormKey(walletId, contact), walletId, draft, () => {
    amount.setText("");
    setSubtractFee(subtractFeeByDefault());
    setNote("");
    setAdvanced(DEFAULT_ADVANCED);
  });
  const { phase } = sheets;
  const reviewButton = useRef<HTMLButtonElement>(null);
  const amountField = useRef<HTMLInputElement>(null);
  const backToForm = () => (reviewButton.current?.disabled === false ? reviewButton.current : amountField.current);

  const { duffs } = amount;
  // "From" (C-funding D-C5, P16): the Dash wallet each time the form opens, never remembered.
  const discreet = useUi((s) => s.discreet);
  const pools = usePoolBalances(walletId);
  const spendsOff = shieldedSpendsOff(useShieldedStatus(walletId).data);
  const [chosen, setChosen] = useState<PaySource>("core");
  const coreAvailable = useAvailable(walletId, advanced, sheets.held);
  const rows = fromRows({ coreDuffs: coreAvailable, shielded: pools.shielded, platform: pools.platform, amount: (d) => shown.withUnit(d), discreet, acceptsToo: acceptsToo(detail.data, accepted), live: liveSources(network.kind), shieldedSpendsOff: spendsOff });
  // The choice stays the user's: a source that can no longer pay holds Review with its reason.
  const source = chosen;
  const activity = usePlatformActivity(walletId);
  const basis = `${pools.platform}|${activity.dataUpdatedAt}`;
  const balance = useBalancePay({ walletId, identity, contact, source, duffs, settled: amount.settled && !amount.fiat.error, shown, blocked: chosenReason(rows, chosen), basis });
  // A payment whose reply was lost holds the contact until a later read settles it (P6).
  const [lostFound, setLostFound] = useState<{ text: string; resolved: boolean } | null>(null);
  const { flow } = balance;
  const settleLost = useCallback(async () => {
    const r = await flow.recheck();
    if (r) setLostFound(resolveText(r));
  }, [flow]);
  const lost = flow.unresolved !== null;
  useEffect(() => {
    if (lost) void settleLost();
  }, [lost, settleLost]);
  const lostHeld = !lockHeld && lost;
  const held = lockHeld || lostHeld;
  const shownFound = found ?? lostFound;
  const shownBanner: Banner | null = lostHeld ? { kind: "paymentLock", text: t("pay.contact.banner.paymentLockPlatform"), txid: "", source: "platform" } : banner;
  const recheck = () => {
    setLostFound(null);
    void settleLost();
    void detail.refetch();
  };
  const recipient: ContactRecipient | null =
    typeof duffs === "number" && duffs > 0 ? { identity, contact, amount: duffs, subtractFee, note: note.trim() || null } : null;
  const spec = draftSpec(recipient, advanced);
  const available = coreAvailable;
  const { estimate, general, fieldError, ready: estimated, offer } = useEstimate(walletId, draft, spec, advanced, locked || held || sheets.held || balance.active, shown);
  const ready = estimated && amount.settled && !amount.fiat.error;
  const max = useMax(walletId, advanced, shown, (text) => {
    amount.setText(text);
    setSubtractFee(true);
  });

  const review = () => {
    if (ready && estimate.data && spec) sheets.review(confirmedOf(spec.recipient, estimate.data, spec.change), spec);
  };

  return (
    <>
      <form
        className="pay-form"
        data-testid="pay-contact-form"
        aria-label={t("pay.contact.form", { name: payee.name })}
        onSubmit={(e) => {
          e.preventDefault();
          if (balance.active) balance.review();
          else review();
        }}
      >
        <section className="pay-card card-l1 pay-payee-card" data-testid="pay-payee">
          <span className="pay-payee">
            <Avatar letters={payee.letters} size="pane" identity={payee.contact} />
            <span className="pay-payee-names">
              <span className="pay-payee-name" data-testid="pay-payee-name">
                {payee.name}
              </span>
              {payee.sub && <span className="contact-row-username">{payee.sub}</span>}
            </span>
          </span>
          <DashButton variant="tintedBlue" size="small" data-testid="pay-payee-change" onClick={() => ui.set({ payContact: null })}>
            {t("pay.contact.change")}
          </DashButton>
        </section>
        {shownBanner && <PayBanner banner={shownBanner} contact={contact} checking={lostHeld ? flow.checking : resolve.isPending} onCheck={lostHeld ? recheck : check} retry={() => void detail.refetch()} />}
        <ActionLine ok={shownFound?.resolved ?? true} text={shownFound?.text ?? null} testId="pay-lock-resolve" />
        {!held && (
          <section className="pay-card card-l1">
            <FromStep rows={rows} source={source} onChange={setChosen} />
          </section>
        )}
        {!held && balance.active && (
          <>
            <section className="pay-card card-l1">
              <BalanceAmount shown={shown} amount={amount} inputRef={amountField} />
              <FormField label={t("pay.contact.note")} hint={t("pay.contact.noteHint")}>
                {(p) => <input {...p} data-testid="pay-note" value={note} autoComplete="off" onChange={(e) => setNote(e.target.value)} />}
              </FormField>
            </section>
            <PayFooter form="contact" problem={balance.problem} fee={balance.fee} shown={shown} ready={balance.ready} incomplete={balance.incomplete} reviewRef={reviewButton} />
          </>
        )}
        {!held && !balance.active && (
          <>
            <section className="pay-card card-l1">
              <AmountFields
                shown={shown}
                amount={amount}
                max={max}
                error={fieldError("amount")}
                available={available}
            coins={advanced.coins}
                subtractFee={subtractFee}
                onSubtractFee={setSubtractFee}
                inputRef={amountField}
              />
              <FormField label={t("pay.contact.note")} hint={t("pay.contact.noteHint")}>
                {(p) => <input {...p} data-testid="pay-note" value={note} autoComplete="off" onChange={(e) => setNote(e.target.value)} />}
              </FormField>
            </section>
            <CoinControl walletId={walletId} network={network} advanced={advanced} onChange={setAdvanced} payAmount={typeof duffs === "number" ? duffs : null} />
            <PayFooter form="contact" problem={general} offer={offer === null || !amount.settled ? null : { amount: offer, accept: () => setSubtractFee(true) }} fee={estimate.data && spec ? estimate.data.fee : null} shown={shown} ready={ready} incomplete={!spec} reviewRef={reviewButton} />
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
          onCancel={sheets.cancel}
          onOutcome={sheets.outcome}
          onSend={sheets.begin}
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
      {(balance.confirmed !== null || balance.flow.step !== "confirm") && (
        <BalancePaySheet
          flow={balance.flow}
          quote={balance.confirmed}
          name={payee.name}
          sub={payee.sub ?? shortId(payee.contact)}
          before={activity.data}
          note={note.trim() || null}
          shown={shown}
          onClose={(paid) => {
            const unknown = balance.flow.outcome?.state === "pending" ? balance.flow.outcome.reference : null;
            balance.close();
            // The contact is locked from here, whatever the next read takes (P6).
            if (unknown !== null)
              client.setQueryData<ContactDetailInfo>([...keys.contactsAll(walletId), "detail", identity, contact], (d) =>
                d && !d.paymentLock ? { ...d, paymentLock: { txid: unknown, since: Math.floor(Date.now() / 1000), source: "platform", reference: unknown } } : d,
              );
            if (paid) {
              amount.setText("");
              setNote("");
              // The next payment opens on the Dash wallet again (P16).
              setChosen("core");
            }
            refreshAfter(client, walletId, identity, ["contacts"]);
            backToForm()?.focus();
          }}
        />
      )}
      {phase.kind === "sending" && <SendingSheet sending={phase.sending} network={network} />}
      {phase.kind === "result" && (
        <ResultSheet
          outcome={phase.outcome}
          network={network}
          draft={draft}
          returnFocus={backToForm}
          retrying={phase.retrying}
          onRetry={sheets.retry}
          onDone={sheets.done}
        />
      )}
    </>
  );
}

/** Why the payee cannot be paid now, and what to do (D6, D7). */
function PayBanner({ banner, contact, checking, onCheck, retry }: { banner: Banner; contact: string; checking: boolean; onCheck: () => void; retry: () => void }) {
  const t = useT();
  if (banner.kind === "loading") return <LoadingState testId="pay-contact-loading" text={t("pay.contact.reading")} />;
  let action = null;
  switch (banner.kind) {
    case "readFailed":
      action = (
        <DashButton variant="tintedBlue" size="small" onClick={retry}>
          {t("pay.contact.retry")}
        </DashButton>
      );
      break;
    case "enable":
      action = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-banner-contacts" onClick={() => navigate("contacts", { contact })}>
          {t("pay.contact.openInContacts")}
        </DashButton>
      );
      break;
    case "paymentLock":
      action = (
        <span className="add-contact-actions">
          {/* A payout's reference is no Core transaction: there is none to show yet. */}
          {banner.source === "core" && (
            <DashButton key="show" variant="tintedGray" size="small" data-testid="pay-banner-show" onClick={() => navigate("activity", { detail: banner.txid })}>
              {t("pay.contact.showTransaction")}
            </DashButton>
          )}
          <DashButton key="check" variant="tintedBlue" size="small" data-testid="pay-banner-check" disabled={checking} onClick={onCheck}>
            {checking ? t("pay.contact.checking") : t("pay.contact.checkAgain")}
          </DashButton>
        </span>
      );
      break;
    case "mixingOnly":
      action = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-banner-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
          {t("pay.contact.unlock")}
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
