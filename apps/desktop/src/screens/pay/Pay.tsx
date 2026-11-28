import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { ContactRecipient } from "../../api/generated/ContactRecipient";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { keys, useBackend, useLockStatus, usePlatformActivity, usePoolBalances, useShieldedStatus, useWallets } from "../../api/queries";
import { ui, useUi, type PayPrefill } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { refreshAfter, useContactDetail, useContactsOwner, useHasKeys } from "../../contacts/queries";
import { shortId } from "../../contacts/model";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { shieldedSpendsOff } from "../../shielded/spends";
import { ShieldedPayButton } from "../ShieldedSheet";
import { AddressBookPicker } from "../tools/AddressBook";
import { Outcome as ActionLine } from "../tools/common";
import { CoinControl } from "./CoinControl";
import { ConfirmSheet } from "./ConfirmSheet";
import { FoundUser } from "./FoundUser";
import { MoreRecipient, type MoreRow } from "./MoreRecipients";
import { PayBanner } from "./PayBanner";
import { BalanceAmount, BalancePaySheet, FromStep, useBalancePay } from "./PayFromBalance";
import { HeldPlatformSend, PayFromPlatform } from "./PayFromPlatform";
import { RecipientField } from "./RecipientField";
import { ResultSheet, SendingSheet } from "./ResultSheet";
import { UnsignedPsbt } from "./UnsignedPsbt";
import { acceptsToo, blocks, payBanner, payeeOf, resolveText, type Banner, type Payee } from "./contact";
import { useResolveLock } from "./contactQueries";
import { AmountFields, draftSpec, FormField, PayFooter, payFormKey, useAvailable, useEstimate, useMax, usePayAmount, usePaySheets } from "./form";
import { chosenReason, fromRows, liveSources, type PaySource } from "./fromBalance";
import { addressProblem, confirmedOf, DEFAULT_ADVANCED, multiRecipientPay, recipientOf, reviewBlockedReason, type Advanced } from "./model";
import { editRecipient, type RecipientEntry, type FoundHit } from "./recipient";
import { PayScanModel, type Scanned } from "./scan";
import { useDraft } from "./useDraft";

type PickedPayee = { payee: Payee; unverified: boolean };

export function Pay({
  prefill, coins, walletId, network,
}: {
  prefill: PayPrefill | null;
  coins: string[] | null;
  walletId: string;
  network: NetworkInfo;
}) {
  const t = useT();
  const b = useBackend();
  const client = useQueryClient();
  const shown = useDisplay(network);
  const instance = useId();
  const lockState = useLockStatus().data?.state;
  const locked = lockState === "locked";
  const draft = useDraft(walletId, locked);
  const watchOnly = useWallets().data?.find((w) => w.walletId === walletId)?.watchOnly;
  const { owner, retry: retryOwner } = useContactsOwner(walletId);
  const selection = useUi((s) => s.payContact?.walletId === walletId ? s.payContact : null);
  const contact = selection?.contact ?? null;
  const identity = selection?.identity ?? null;
  const [shownAs, setShownAs] = useState<PickedPayee | null>(null);
  const [found, setFound] = useState<FoundHit | null>(null);
  const lastSearch = useRef("");
  const [entry, setEntry] = useState<RecipientEntry>(() => ({ text: prefill?.address ?? "", address: null, revision: 0 }));
  const entryNow = useRef(entry);
  const updateEntry = (next: RecipientEntry) => {
    entryNow.current = next;
    setEntry(next);
  };

  const [validation, setValidation] = useState<{ text: string; problem: string } | null>(null);
  const validate = useCallback((text: string, problem: string | null) => {
    setValidation(problem === null ? null : { text, problem });
  }, []);
  const inputProblem = validation?.text === entry.text ? validation.problem : null;

  const rawAmount = usePayAmount(shown, prefill?.amount ? shown.full(prefill.amount) : "");
  const amountOrigin = useRef<"none" | "user" | "link" | "coreMax">(prefill?.amount ? "link" : "none");
  const setAmountFrom = useCallback((origin: "none" | "user" | "link" | "coreMax", text: string) => {
    amountOrigin.current = text.trim() === "" ? "none" : origin;
    rawAmount.setText(text);
  }, [rawAmount.setText]);
  const amount: ReturnType<typeof usePayAmount> = {
    ...rawAmount,
    setText: (next) => {
      if (typeof next === "string") return setAmountFrom("user", next);
      rawAmount.setText((prev) => {
        const value = next(prev);
        amountOrigin.current = value.trim() === "" ? "none" : "user";
        return value;
      });
    },
    fiat: {
      ...rawAmount.fiat,
      onChange: (text: string) => {
        amountOrigin.current = text.trim() === "" ? "none" : "user";
        rawAmount.fiat.onChange(text);
      },
    },
  };
  const [label, setLabel] = useState(prefill?.label ?? "");
  const labelOrigin = useRef<"none" | "user" | "link">(prefill?.label ? "link" : "none");
  const setLabelFrom = (origin: "none" | "user" | "link", value: string) => {
    labelOrigin.current = value === "" ? "none" : origin;
    setLabel(value);
  };
  const [message, setMessage] = useState(prefill?.message ?? "");
  const messageOrigin = useRef<"none" | "user" | "link">(prefill?.message ? "link" : "none");
  const setMessageFrom = (origin: "none" | "user" | "link", value: string) => {
    messageOrigin.current = value === "" ? "none" : origin;
    setMessage(value);
  };
  const [linkUsername, setLinkUsername] = useState(prefill?.username ?? null);
  const [note, setNote] = useState("");
  const [source, setSource] = useState<PaySource>("core");
  const sourceNow = useRef(source);
  sourceNow.current = source;
  const subtractFeeByDefault = () => ui.get().prefs.subtractFeeByDefault;
  const [subtractFee, setSubtractFeeFirst] = useState(subtractFeeByDefault);
  const [advanced, setAdvanced] = useState<Advanced>(coins?.length ? { ...DEFAULT_ADVANCED, coins: "selected", selected: coins } : DEFAULT_ADVANCED);
  const [more, setMore] = useState<MoreRow[]>([]);
  const multi = contact === null && found === null && multiRecipientPay(network);
  const nextRow = useRef(1);
  const setSubtractFee = useCallback((on: boolean) => {
    setSubtractFeeFirst(on);
    if (on) setMore((rows) => rows.map((r) => (r.subtractFee ? { ...r, subtractFee: false } : r)));
  }, []);
  const [picking, setPicking] = useState(false);
  const reviewButton = useRef<HTMLButtonElement>(null);
  const recipientField = useRef<HTMLInputElement>(null);
  const amountField = useRef<HTMLInputElement>(null);
  const restoreAmountFocus = useRef(false);
  const bindAmountField = useCallback((input: HTMLInputElement | null) => {
    if (input === null) restoreAmountFocus.current = document.activeElement === amountField.current;
    else {
      if (restoreAmountFocus.current && document.activeElement === document.body) input.focus({ preventScroll: true });
      restoreAmountFocus.current = false;
    }
    amountField.current = input;
  }, []);
  const retryButton = useRef<HTMLButtonElement>(null);
  const psbtButton = useRef<HTMLButtonElement>(null);
  const psbtSummary = useRef<HTMLElement>(null);
  const openedBy = useRef<"review" | "psbt">("review");

  const [acceptedContact, setAcceptedContact] = useState<string | null>(null);
  const [checked, setChecked] = useState<string | null>(null);
  const [lostFound, setLostFound] = useState<{ text: string; resolved: boolean } | null>(null);
  const [reopen, setReopen] = useState<{ spec: string; at: number } | null>(null);
  const resolve = useResolveLock(walletId, identity, contact);
  const resetRecipient = (text: string) => {
    updateEntry(editRecipient(entryNow.current, text));
    setLinkUsername(null);
    if (amountOrigin.current === "link") setAmountFrom("none", "");
    if (labelOrigin.current === "link") setLabelFrom("none", "");
    if (messageOrigin.current === "link") setMessageFrom("none", "");
    setFound(null);
    setShownAs(null);
    setAcceptedContact(null);
    setChecked(null);
    setLostFound(null);
    setReopen(null);
  };
  const changeRecipient = (text: string) => {
    resetRecipient(text);
    if (contact !== null) {
      setNote("");
      setSource("core");
      setAdvanced(DEFAULT_ADVANCED);
      setSubtractFee(subtractFeeByDefault());
    }
    ui.set({ payContact: null, payPrefill: null });
  };
  const clearAddress = () => {
    changeRecipient("");
    setLabelFrom("none", "");
    setMessageFrom("none", "");
  };
  const clearAddressForm = () => {
    resetRecipient("");
    setLabelFrom("none", "");
    setMessageFrom("none", "");
    setMore([]);
    ui.set({ payPrefill: null });
  };
  const focusRecipient = () => requestAnimationFrame(() => recipientField.current?.focus());
  const pickContact = (picked: PickedPayee) => {
    if (owner.kind !== "identity") return;
    clearAddress();
    setMore([]);
    setAdvanced(DEFAULT_ADVANCED);
    setSubtractFee(subtractFeeByDefault());
    setShownAs(picked);
    ui.set({ payContact: { walletId, identity: owner.identity, contact: picked.payee.contact }, payCoins: null });
    requestAnimationFrame(() => amountField.current?.focus());
  };
  const pickUser = (hit: FoundHit, input?: string) => {
    if (owner.kind !== "identity") return;
    if (input !== undefined) {
      const current = entryNow.current;
      if (current.address !== null || current.text.trim() !== input || scanner.busy || contact !== null || found !== null) return;
    }
    if (hit.relation === "contact") {
      pickContact({
        payee: payeeOf({ contact: hit.identity, alias: null, displayName: hit.displayName, username: hit.username }),
        unverified: hit.unverified,
      });
    } else {
      lastSearch.current = input !== undefined ? "" : entryNow.current.text;
      clearAddress();
      setFound(hit);
    }
  };
  const fillRequest = (s: Scanned) => {
    if (s.amount !== null && (amountOrigin.current === "none" || amountOrigin.current === "link")) setAmountFrom("link", shown.full(s.amount));
    if (s.label) setLabelFrom("link", s.label);
    if (s.message) setMessageFrom("link", s.message);
  };
  const onScanned = useRef<(s: Scanned) => void>(() => undefined);
  onScanned.current = (s) => {
    changeRecipient(s.address);
    fillRequest(s);
    focusRecipient();
  };
  const scanner = useModel(useMemo(() => new PayScanModel(b, network.dirName, (s) => onScanned.current(s)), [b, network.dirName]));
  useEffect(() => () => scanner.dispose(), [scanner]);

  const sheets = usePaySheets(payFormKey(walletId, contact ?? undefined), walletId, draft, () => {
    setAmountFrom("none", "");
    setSubtractFee(subtractFeeByDefault());
    setAdvanced(DEFAULT_ADVANCED);
    if (contact !== null) setNote("");
    else clearAddressForm();
  }, contact === null ? "link" : null);
  const { phase } = sheets;
  const backToForm = () => {
    if (openedBy.current === "psbt") return psbtButton.current?.disabled === false ? psbtButton.current : psbtSummary.current;
    return retryButton.current ?? (reviewButton.current?.disabled === false ? reviewButton.current : contact ? amountField.current : recipientField.current);
  };
  const address = entry.address;
  const shieldedStatus = useShieldedStatus(walletId).data ?? null;
  const shieldedRoute = shieldedStatus?.active ? shieldedStatus : null;
  const spendsOff = shieldedSpendsOff(shieldedStatus);
  const shieldedTo = address?.route === "shielded" && shieldedRoute !== null;
  const addrError = address?.route === "shielded" && !shieldedTo ? addressProblem({ kind: "shielded" }) : null;
  const platform = address?.route === "platform";

  const detail = useContactDetail(walletId, identity ?? "", contact);
  const hasKeys = useHasKeys(walletId, identity);
  const accepted = acceptedContact !== null && acceptedContact === contact;
  const accepts = acceptsToo(detail.data, accepted);
  const summary = detail.data?.summary;
  const payee = contact === null ? null : summary
    ? { payee: payeeOf(summary), unverified: summary.unverified }
    : shownAs?.payee.contact === contact ? shownAs
    : { payee: payeeOf({ contact, alias: null, displayName: null, username: null }), unverified: false };
  const banner = contact === null ? null : payBanner({
    detail: detail.data,
    readError: detail.data === undefined && detail.isError ? errorMessage(detail.error) : null,
    watchOnly, lock: lockState, hasKeys, accepted,
  });
  const lockHeld = blocks(banner);
  const lockTxid = detail.data?.paymentLock?.txid ?? null;
  const check = () => {
    if (identity === null || contact === null) return;
    setChecked(lockTxid);
    resolve.mutate({ identity, contact });
  };
  const checkedLock = resolve.variables?.contact === contact && resolve.variables?.identity === identity && (lockTxid === null || lockTxid === checked);
  const resolvedLock = !checkedLock ? null
    : resolve.isError ? { text: errorMessage(resolve.error), resolved: false }
    : resolve.data ? resolveText(resolve.data)
    : null;

  const discreet = useUi((s) => s.discreet);
  const available = useAvailable(walletId, advanced, sheets.held);
  const pools = usePoolBalances(walletId);
  const rows = fromRows({ coreDuffs: available, shielded: pools.shielded, platform: pools.platform, amount: (d) => shown.withUnit(d), discreet, acceptsToo: accepts, shieldedHold: pools.shieldedHold, live: liveSources(network.kind), shieldedSpendsOff: spendsOff });
  const activity = usePlatformActivity(walletId);
  const { duffs } = amount;
  const revision = `${instance}:${entry.revision}`;
  const balance = useBalancePay({
    walletId, identity, contact, source, duffs, shown, blocked: chosenReason(rows, source),
    settled: amount.settled && !amount.fiat.error && !scanner.busy,
    basis: `${pools.platform}|${activity.dataUpdatedAt}`,
    recipientRevision: revision,
  });
  const { flow } = balance;
  const balanceOpen = flow !== null && (balance.confirmed !== null || flow.step !== "confirm" || flow.running);
  const lost = !!flow?.unresolved;
  const currentFlow = useRef(flow);
  currentFlow.current = flow;
  const settleLost = useCallback(async () => {
    const r = await flow?.recheck();
    if (r && flow === currentFlow.current) setLostFound(resolveText(r));
  }, [flow]);
  useEffect(() => {
    if (lost) void settleLost();
  }, [lost, settleLost]);
  const lostHeld = !lockHeld && lost;
  const held = lockHeld || lostHeld;
  const shownFound = resolvedLock ?? lostFound;
  const shownBanner: Banner | null = lostHeld ? { kind: "paymentLock", text: t("pay.contact.banner.paymentLockPlatform"), txid: "", source: "platform" } : banner;
  const recheck = () => {
    setLostFound(null);
    void settleLost();
    void detail.refetch();
  };

  const recipient: ContactRecipient | ReturnType<typeof recipientOf> | null = !amount.settled || typeof duffs !== "number" || duffs <= 0
    ? null
    : contact !== null && identity !== null
      ? { identity, contact, amount: duffs, subtractFee, note: note.trim() || null }
      : address?.route === "core" ? recipientOf({ address: address.address, amount: duffs, subtractFee, label, message }) : null;
  const rowsSettled = more.every((r) => r.settled);
  const moreRecipients = more.every((r) => r.recipient !== null) ? more.map((r) => r.recipient!) : null;
  const spec = draftSpec(recipient, advanced, multi ? (rowsSettled ? moreRecipients : null) : []);
  const hold = locked || held || sheets.held || balance.active || scanner.busy;
  const { estimate, general, notice: estimateNotice, fieldError, ready: estimated, offer } = useEstimate(walletId, draft, spec, advanced, hold, shown, revision);
  const ready = estimated && !amount.fiat.error;
  const notice = sheets.notice ?? estimateNotice;
  const specKey = spec && JSON.stringify([revision, spec]);
  const tryAgain = () => {
    const again = sheets.notice !== null;
    sheets.dismissNotice();
    if (!specKey || hold) return;
    if (again) setReopen({ spec: specKey, at: Date.now() });
    void estimate.refetch();
  };
  const max = useMax(
    walletId,
    advanced,
    shown,
    (text) => {
      setAmountFrom("coreMax", text);
      setSubtractFee(true);
    },
    multi ? more.reduce((n, r) => n + (r.amount ?? 0), 0) : 0,
    multi && !rowsSettled,
    revision,
    () => entryNow.current.address?.route !== "platform" && sourceNow.current === "core",
  );
  const review = (from: "review" | "psbt") => {
    if (!ready || !estimate.data || !spec) return;
    openedBy.current = from;
    sheets.dismissNotice();
    sheets.review(confirmedOf(spec.recipient, estimate.data, spec.change, spec.more), spec, contact === null && (from === "psbt" || watchOnly === true));
  };
  useEffect(() => {
    if (!reopen) return;
    if (reopen.spec !== specKey || estimate.isError || hold) setReopen(null);
    else if (ready && estimate.dataUpdatedAt >= reopen.at) {
      setReopen(null);
      review("review");
    }
  });

  return (
    <>
      <form className="pay-form" data-testid={contact === null ? "pay-address-form" : "pay-contact-form"} aria-label={t("pay.recipient.form")} onSubmit={(e) => {
        e.preventDefault();
        if (balance.active) {
          if (!held && balance.ready) balance.review();
        } else if (!platform) review("review");
      }}>
        <section className="pay-card card-l1">
          <RecipientField
            walletId={walletId}
            network={network.dirName}
            owner={owner}
            retryOwner={retryOwner}
            inputRef={recipientField}
            text={entry.text}
            address={address}
            contact={payee}
            found={found}
            disabled={phase.kind !== "editing" || balanceOpen}
            addressError={addrError ?? fieldError("address")}
            shieldedHint={shieldedTo && !spendsOff}
            scanner={scanner}
            onText={changeRecipient}
            onValidation={validate}
            onAddress={(next, input) => {
              const current = entryNow.current;
              if (current.address !== null || current.text.trim() !== input || scanner.busy || contact !== null || found !== null) return;
              updateEntry({ ...current, text: next.address, address: next });
              if (next.route === "platform" && amountOrigin.current === "coreMax") setAmountFrom("none", "");
              fillRequest(next);
              if (next.username !== null) setLinkUsername(next.username);
            }}
            onContact={pickContact}
            onFound={pickUser}
            onAddressBook={() => { scanner.dismiss(); setPicking(true); }}
            onClear={() => { clearAddress(); focusRecipient(); }}
          />
          {linkUsername !== null && <p className="placeholder-note" data-testid="pay-username">{t("pay.toAddress.linkUsername", { username: `@${linkUsername}` })}</p>}
          {shieldedTo && !scanner.busy && (
            <div className="pay-shielded-route">
              <ShieldedPayButton walletId={walletId} status={shieldedRoute} to={address.address} text={typeof duffs === "number" && duffs > 0 ? amount.text.trim() : ""} />
            </div>
          )}
        </section>
        {contact && shownBanner && <PayBanner banner={shownBanner} contact={contact} checking={lostHeld ? !!flow?.checking : resolve.isPending} onCheck={lostHeld ? recheck : check} retry={() => void detail.refetch()} />}
        {contact && notice && !held && !balance.active && <PayBanner banner={notice} contact={contact} checking={estimate.isFetching || reopen !== null} onCheck={check} retry={tryAgain} actionRef={retryButton} />}
        {contact && <ActionLine ok={shownFound?.resolved ?? true} text={shownFound?.text ?? null} testId="pay-lock-resolve" />}
        {found && owner.kind === "identity" && (
          <FoundUser walletId={walletId} identity={owner.identity} hit={found} onPay={(payee) => pickContact({ payee, unverified: found.unverified })} onBack={() => { changeRecipient(lastSearch.current); focusRecipient(); }} />
        )}
        {!held && !found && (
          <>
            <section className="pay-card card-l1">
              {platform ? (
                <PayFromPlatform key={revision} walletId={walletId} network={network} to={address.address} amount={amount} onSent={clearAddressForm} recipientBusy={scanner.busy} recipientRevision={revision} inputRef={bindAmountField} />
              ) : (
                <>
                  {balance.active ? <BalanceAmount shown={shown} amount={amount} inputRef={bindAmountField} /> : (
                    <AmountFields shown={shown} amount={amount} max={max} error={fieldError("amount")} available={available} coins={advanced.coins} subtractFee={subtractFee} onSubtractFee={setSubtractFee} inputRef={bindAmountField} />
                  )}
                  {contact ? (
                    <FormField label={t("pay.contact.note")} hint={t("pay.contact.noteHint")}>
                      {(p) => <input {...p} data-testid="pay-note" value={note} autoComplete="off" onChange={(e) => setNote(e.target.value)} />}
                    </FormField>
                  ) : (
                    <>
                      <FormField label={t("pay.toAddress.label")} hint={t("pay.toAddress.labelHint")}>
                        {(p) => <input {...p} data-testid="pay-label" value={label} autoComplete="off" onChange={(e) => setLabelFrom("user", e.target.value)} />}
                      </FormField>
                      <FormField label={t("pay.toAddress.message")} hint={t("pay.toAddress.messageHint")}>
                        {(p) => <input {...p} data-testid="pay-message" value={message} autoComplete="off" onChange={(e) => setMessageFrom("user", e.target.value)} />}
                      </FormField>
                    </>
                  )}
                </>
              )}
            </section>
            {contact !== null && <section className="pay-card card-l1"><FromStep rows={rows} source={source} onChange={setSource} /></section>}
            {multi && more.map((row, i) => (
              <div key={row.id} hidden={platform}>
                <MoreRecipient
                  row={row}
                  number={i + 2}
                  network={network}
                  shown={shown}
                  shieldedRoute={shieldedRoute !== null && !spendsOff}
                  fieldError={(f) => fieldError(f, i + 1)}
                  onChange={(r, amount, settled) => setMore((rows) => rows.map((x) => (x.id === row.id ? { ...x, recipient: r, amount, settled } : x)))}
                  onSubtractFee={(on) => {
                    if (on) setSubtractFeeFirst(false);
                    setMore((rows) => rows.map((r) => (r.id === row.id ? { ...r, subtractFee: on } : on ? { ...r, subtractFee: false } : r)));
                  }}
                  onRemove={() => setMore((rows) => rows.filter((x) => x.id !== row.id))}
                />
              </div>
            ))}
            {!platform && multi && (
              <div className="pay-more-add">
                <DashButton variant="tintedBlue" size="small" data-testid="pay-add-recipient" onClick={() => setMore((rows) => [...rows, { id: nextRow.current++, recipient: null, amount: null, settled: true, subtractFee: false }])}>
                  {t("pay.more.add")}
                </DashButton>
                <span className="field-hint" data-testid="pay-add-recipient-note">{t("pay.more.testnetOnly")}</span>
              </div>
            )}
            {!platform && !balance.active && (
              <>
                <CoinControl walletId={walletId} network={network} advanced={advanced} onChange={setAdvanced} payAmount={typeof duffs === "number" && more.every((r) => r.amount !== null) ? [duffs, ...more.map((r) => r.amount!)] : null} />
                {contact === null && (
                  <UnsignedPsbt watchOnly={watchOnly === true} why={reviewBlockedReason(ready, [addrError, inputProblem, fieldError("address"), amount.fiat.error, amount.error, fieldError("amount"), max.error, general], !!spec)} onCreate={() => review("psbt")} buttonRef={psbtButton} summaryRef={psbtSummary} />
                )}
              </>
            )}
            {!platform && (
              <PayFooter
                form={contact ? "contact" : "address"}
                reviewLabel={contact === null && watchOnly ? t("pay.toAddress.reviewPsbt") : t("pay.form.review")}
                problem={balance.active ? balance.problem : general}
                offer={balance.active || offer === null || !amount.settled ? null : { amount: offer, accept: () => setSubtractFee(true) }}
                fee={balance.active ? balance.fee : estimate.data && spec ? estimate.data.fee : null}
                shown={shown}
                ready={balance.active ? balance.ready : ready}
                incomplete={balance.active ? balance.incomplete : !spec}
                reviewRef={reviewButton}
              />
            )}
          </>
        )}
      </form>
      {contact === null && !platform && <HeldPlatformSend walletId={walletId} network={network} />}
      {picking && (
        <AddressBookPicker walletId={walletId} network={network} purpose="send" onChoose={(address, name) => {
          changeRecipient(address);
          if (labelOrigin.current === "none" && name) setLabelFrom("link", name);
          setPicking(false);
          focusRecipient();
        }} onClose={() => setPicking(false)} />
      )}
      {phase.kind === "confirm" && (
        <ConfirmSheet
          confirmed={phase.confirmed}
          spec={phase.spec}
          coins={advanced}
          walletId={walletId}
          network={network}
          draft={draft}
          unsigned={phase.unsigned}
          returnFocus={backToForm}
          onCancel={sheets.cancel}
          onOutcome={sheets.outcome}
          onSend={sheets.begin}
          contact={identity !== null && payee ? {
            identity, payee: payee.payee, acceptsToo: accepts,
            onAccepted: () => {
              setAcceptedContact(contact);
              refreshAfter(client, walletId, identity, ["contacts"]);
            },
          } : undefined}
        />
      )}
      {identity !== null && contact !== null && payee && flow && balanceOpen && (
        <BalancePaySheet
          flow={flow}
          quote={balance.confirmed}
          name={payee.payee.name}
          sub={payee.payee.sub ?? shortId(contact)}
          before={activity.data}
          note={note.trim() || null}
          shown={shown}
          retrying={balance.retrying}
          onRetry={() => void balance.retry()}
          onClose={(paid) => {
            const unknown = flow.outcome?.state === "pending" ? flow.outcome.reference : null;
            balance.close();
            if (unknown !== null)
              client.setQueryData<ContactDetailInfo>([...keys.contactsAll(walletId), "detail", identity, contact], (d) =>
                d && !d.paymentLock ? { ...d, paymentLock: { txid: unknown, since: Math.floor(Date.now() / 1000), source: "platform", reference: unknown } } : d,
              );
            if (paid) {
              setAmountFrom("none", "");
              setNote("");
              setSource("core");
            }
            refreshAfter(client, walletId, identity, ["contacts"]);
            backToForm()?.focus();
          }}
        />
      )}
      {phase.kind === "sending" && <SendingSheet sending={phase.sending} network={network} />}
      {phase.kind === "result" && (
        <ResultSheet outcome={phase.outcome} network={network} draft={draft} returnFocus={backToForm} retrying={phase.retrying} onRetry={sheets.retry} onDone={sheets.done} />
      )}
    </>
  );
}
