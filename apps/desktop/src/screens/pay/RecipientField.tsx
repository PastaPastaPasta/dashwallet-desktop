import { useEffect, useId, useRef, useState, type DragEvent, type KeyboardEvent, type RefObject } from "react";
import { useQuery } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import { useBackend } from "../../api/queries";
import { navigate } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { Chip, DashButton, VisuallyHidden } from "../../components/basics";
import { NETWORK_DEBOUNCE_MS, networkPrefix, relationTitle, SEARCH_DEBOUNCE_MS } from "../../contacts/model";
import { useNetworkSearch, useScreenScanMode } from "../../contacts/queries";
import { useT } from "../../i18n";
import { Avatar } from "../contacts/Avatar";
import { noOwner, payeeOf, pickerHits, pickerListing, searchAnnouncement, type Payee } from "./contact";
import { useFrequentContacts, usePayPicker } from "./contactQueries";
import { SETTLE_MS } from "./form";
import { directRecipient, parseRecipientInput, recipientSuggestions, resolveRecipient, type AddressRecipient, type FoundHit } from "./recipient";
import type { PayScanModel } from "./scan";
import type { ContactsOwner } from "../../contacts/model";

type PickedPayee = { payee: Payee; unverified: boolean };

type Props = {
  walletId: string;
  network: string;
  owner: ContactsOwner;
  retryOwner: () => void;
  inputRef: RefObject<HTMLInputElement | null>;
  text: string;
  address: AddressRecipient | null;
  contact: PickedPayee | null;
  found: FoundHit | null;
  disabled: boolean;
  addressError: string | null;
  shieldedHint: boolean;
  onText: (text: string) => void;
  onValidation: (text: string, problem: string | null) => void;
  onAddress: (address: AddressRecipient, input: string) => void;
  scanner: PayScanModel;
  onContact: (contact: PickedPayee) => void;
  onFound: (hit: FoundHit) => void;
  onAddressBook: () => void;
  onClear: () => void;
};

export function RecipientField({
  walletId, network, owner, retryOwner, inputRef, text, address, contact, found, disabled,
  addressError, shieldedHint, onText, onValidation, onAddress, scanner, onContact, onFound, onAddressBook, onClear,
}: Props) {
  const t = useT();
  const b = useBackend();
  const id = useId();
  const [popup, setPopup] = useState<"search" | "contacts" | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const listbox = useRef<HTMLDivElement>(null);
  const selectedUser = contact ?? (found && {
    payee: payeeOf({ contact: found.identity, alias: null, displayName: found.displayName, username: found.username }),
    unverified: found.unverified,
  });
  const close = () => {
    setPopup(null);
    setActive(null);
  };

  const scanMode = useScreenScanMode();
  const scan = scanner.state;
  const imageOf = (data: DataTransfer | null) => [...(data?.files ?? [])].find((f) => f.type.startsWith("image/"));
  const dropImage = (e: DragEvent<HTMLElement>) => {
    const file = imageOf(e.dataTransfer);
    if (!file || disabled) return;
    e.preventDefault();
    void scanner.imageFile(file);
  };

  const parsed = parseRecipientInput(text);
  const identity = owner.kind === "identity" ? owner.identity : null;
  const check = directRecipient(parsed) && address === null && !selectedUser && !disabled ? parsed.text : "";
  const checked = useDebounced(check, SETTLE_MS);
  const resolution = useQuery({
    queryKey: ["pay-recipient", walletId, network, checked] as const,
    queryFn: () => resolveRecipient(b, walletId, network, parseRecipientInput(checked)),
    enabled: check !== "" && check === checked && (parsed.kind !== "userLink" || identity !== null),
    retry: false,
    staleTime: parsed.kind === "userLink" ? 0 : Infinity,
    gcTime: 0,
  });
  const resolved = check && check === checked && !resolution.isFetching && !resolution.isError ? resolution.data : null;
  useEffect(() => {
    if (resolved?.kind === "address") onAddress(resolved, check);
    if (resolved?.kind === "user") onFound(resolved.hit);
  }, [resolved, check, onAddress, onFound]);

  const search = parsed.kind === "name" && !selectedUser ? parsed.text : "";
  const localText = useDebounced(search.replace(/^@/, ""), SEARCH_DEBOUNCE_MS);
  const prefix = networkPrefix(search.replace(/^@/, ""));
  const lookup = useDebounced(prefix, NETWORK_DEBOUNCE_MS);
  const browsing = popup === "contacts";
  const local = usePayPicker(walletId, browsing || search ? identity : null, browsing ? "" : localText);
  const frequent = useFrequentContacts(walletId, browsing ? identity : null);
  const remote = useNetworkSearch(walletId, identity ?? "", identity && lookup === prefix ? lookup : null);
  const listing = browsing || localText === search.replace(/^@/, "") ? pickerListing({ data: local.data, placeholder: local.isPlaceholderData, failed: local.isError }) : null;
  const remoteReady = prefix === null || (lookup === prefix && !remote.isFetching && !remote.isPending);
  const hits = remoteReady && !remote.isError && !browsing ? remote.data : undefined;
  const networkHits = listing ? pickerHits(hits, listing) : null;
  const options = listing ? recipientSuggestions(listing, hits) : [];
  const open = !disabled && (browsing || (!selectedUser && !address && popup === "search" && search !== "" && identity !== null));

  const recent = open && browsing ? (frequent.data ?? []) : [];
  const optionRows = [
    ...recent.map((c) => ({ key: `recent-${c.contact}`, payee: payeeOf(c), unverified: c.unverified, reason: null, group: "recent", pick: () => onContact({ payee: payeeOf(c), unverified: c.unverified }) })),
    ...options.map((o) => {
      if (o.kind === "contact") {
        const { payee } = o;
        return { key: payee.contact, payee, unverified: payee.unverified, reason: payee.reason, group: o.group, pick: () => onContact({ payee, unverified: payee.unverified }) };
      }
      const { hit } = o;
      const payee = payeeOf({ contact: hit.identity, alias: null, displayName: hit.displayName, username: hit.username });
      return { key: hit.identity, payee, unverified: hit.unverified, reason: hit.relation === "stranger" ? null : relationTitle(hit.relation), group: "network", pick: () => onFound(hit) };
    }),
  ];

  useEffect(() => {
    if (!popup) return;
    const dismiss = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) close();
    };
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [popup]);

  const select = (row: (typeof optionRows)[number]) => {
    scanner.dismiss();
    close();
    row.pick();
  };
  const namesFinished = !browsing && search && ((identity === null && owner.kind !== "starting") || (listing && remoteReady && !remote.isError && !local.isError && options.length === 0));
  const announce = !open || browsing ? null : searchAnnouncement({
    network: prefix !== null,
    networkState: !listing || !remoteReady ? "loading" : remote.isError ? "error" : "done",
    hits: networkHits?.count ?? 0,
    none: namesFinished && prefix !== null ? "anyone" : null,
  });
  const contactLinkNeedsOwner = parsed.kind === "userLink" && identity === null;
  const invalid = resolved?.kind === "invalid" ? resolved.error
    : check && check === checked && resolution.isError ? errorMessage(resolution.error)
    : contactLinkNeedsOwner ? t("pay.error.noIdentity")
    : namesFinished ? t("pay.error.invalidAddress")
    : null;
  useEffect(() => onValidation(text, invalid), [text, invalid, onValidation]);
  const error = addressError ?? invalid;
  const hasOptions = open && optionRows.length > 0;
  const activeRow = hasOptions ? optionRows.find((row) => row.key === active) : undefined;
  useEffect(() => {
    if (hasOptions && browsing && selectedUser) listbox.current?.focus({ preventScroll: true });
  }, [hasOptions, browsing, selectedUser !== null]);

  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape" && popup) {
      e.preventDefault();
      e.stopPropagation();
      close();
      if (selectedUser) root.current?.querySelector<HTMLButtonElement>('[data-testid="pay-contacts"]')?.focus();
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && (search || browsing)) {
      e.preventDefault();
      if (popup === null) setPopup("search");
      if (optionRows.length === 0) return;
      const index = optionRows.findIndex((row) => row.key === active);
      const next = e.key === "ArrowDown" ? Math.min(index + 1, optionRows.length - 1) : (index < 0 ? optionRows.length - 1 : Math.max(0, index - 1));
      const key = optionRows[next].key;
      setActive(key);
      document.getElementById(`${id}-option-${key}`)?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (hasOptions && (activeRow || optionRows.length === 1)) select(activeRow ?? optionRows[0]);
    }
  };
  const title = t("pay.toAddress.payTo");
  const warning = selectedUser?.unverified ? t("pay.contact.unverified") : null;
  const ownerNote = owner.kind !== "identity" && owner.kind !== "starting" ? noOwner(owner) : null;
  const groupLabels = {
    recent: t("pay.recipient.recent"),
    contacts: t("pay.contact.groups.contacts"),
    requests: t("pay.contact.groups.requests"),
    network: t("pay.recipient.network"),
  };

  return (
    <div className="form-field pay-recipient" data-testid="pay-recipient" ref={root} onBlur={(e) => { if (!root.current?.contains(e.relatedTarget)) close(); }} onDragOver={(e) => {
      if ([...e.dataTransfer.items].some((i) => i.kind === "file" && i.type.startsWith("image/"))) e.preventDefault();
    }} onDrop={dropImage}>
      {selectedUser ? <span id={`${id}-label`}>{title}</span> : <label htmlFor={id}>{title}</label>}
      <div className="pay-recipient-entry">
        <div className={`pay-recipient-control${address || selectedUser ? " is-picked" : ""}`} data-testid={address || selectedUser ? "pay-recipient-chip" : undefined}>
          {selectedUser ? (
            <span className="pay-recipient-person" aria-labelledby={`${id}-label`} data-testid="pay-payee">
              <Avatar letters={selectedUser.payee.letters} identity={selectedUser.payee.contact} />
              <span className="pay-payee-name" data-testid="pay-payee-name">{selectedUser.payee.name}</span>
              {selectedUser.payee.sub && <span className="contact-row-username">{selectedUser.payee.sub}</span>}
              {warning && <Chip tone="warning">{warning}</Chip>}
            </span>
          ) : (
            <input
              id={id}
              ref={inputRef}
              data-testid="pay-address"
              value={text}
              role="combobox"
              autoComplete="off"
              spellCheck={false}
              placeholder={t("pay.recipient.placeholder")}
              aria-autocomplete="list"
              aria-controls={hasOptions ? `${id}-options` : undefined}
              aria-expanded={hasOptions}
              aria-activedescendant={activeRow ? `${id}-option-${activeRow.key}` : undefined}
              aria-invalid={!!error}
              aria-describedby={`${id}-feedback`}
              disabled={disabled}
              onChange={(e) => {
                scanner.dismiss();
                setPopup("search");
                setActive(null);
                onText(e.target.value);
              }}
              onFocus={() => {
                if (search && popup === null) setPopup("search");
              }}
              onPaste={(e) => {
                const file = imageOf(e.clipboardData);
                if (!file) return;
                e.preventDefault();
                close();
                void scanner.imageFile(file);
              }}
              onKeyDown={onKey}
            />
          )}
          {(address || selectedUser) && (
            <DashButton variant="plainBlue" size="small" data-testid={contact ? "pay-payee-change" : "pay-recipient-clear"} disabled={disabled} onClick={() => {
              close();
              scanner.dismiss();
              onClear();
            }}>
              {t("pay.recipient.clear")}
            </DashButton>
          )}
        </div>
        <div className="pay-recipient-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="pay-contacts" disabled={disabled} aria-expanded={open && browsing} aria-controls={hasOptions && browsing ? `${id}-options` : undefined} onClick={() => {
            if (open && browsing) return close();
            scanner.dismiss();
            inputRef.current?.focus();
            setActive(null);
            setPopup("contacts");
          }}>
            {t("pay.recipient.contacts")}
          </DashButton>
          <DashButton variant="tintedBlue" size="small" data-testid="pay-address-book" disabled={disabled} onClick={() => { close(); onAddressBook(); }}>
            {t("pay.toAddress.addressBook")}
          </DashButton>
          <DashButton variant="tintedBlue" size="small" data-testid="pay-scan-image" disabled={disabled || scanner.busy} onClick={() => { close(); void scanner.openImage(); }}>
            {t("pay.scan.openImage")}
          </DashButton>
          {scanMode.data === "picker" && (scan.kind === "capturing" ? (
            <DashButton key="cancel" variant="plainBlue" size="small" data-testid="pay-scan-screen-cancel" onClick={() => scanner.cancelScreen()}>{t("pay.scan.cancel")}</DashButton>
          ) : (
            <DashButton key="screen" variant="plainBlue" size="small" data-testid="pay-scan-screen" disabled={disabled || scanner.busy} onClick={() => { close(); void scanner.screen(); }}>{t("pay.scan.screen")}</DashButton>
          ))}
        </div>
        {open && (
          <div className="pay-recipient-popover" data-testid="pay-recipient-popover">
            {hasOptions && <div
              id={`${id}-options`}
              ref={listbox}
              role="listbox"
              tabIndex={0}
              onKeyDown={onKey}
              aria-activedescendant={activeRow ? `${id}-option-${activeRow.key}` : undefined}
              aria-label={t(browsing ? "pay.recipient.contacts" : "pay.recipient.suggestions")}
            >
              {Object.entries(groupLabels).map(([group, label]) => {
                const groupRows = optionRows.filter((row) => row.group === group);
                return groupRows.length > 0 && (
                  <div key={group} role="group" aria-label={label} data-testid={group === "recent" ? "pay-frequent" : `pay-picker-${group}`}>
                    <div className="pay-option-heading" aria-hidden="true">{label}</div>
                    {groupRows.map((row) => (
                      <div
                        key={row.key}
                        id={`${id}-option-${row.key}`}
                        role="option"
                        aria-selected={activeRow?.key === row.key}
                        data-testid={group === "recent" ? "pay-frequent-chip" : group === "network" ? "network-row" : "pay-picker-row"}
                        data-contact={row.payee.contact}
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => select(row)}
                      >
                        <Avatar letters={row.payee.letters} identity={row.payee.contact} />
                        <span>
                          <span className={group === "recent" ? "pay-frequent-name" : "contact-row-name"}>{row.payee.name}</span>
                          {row.payee.sub && <span className="contact-row-username"> {row.payee.sub}</span>}
                        </span>
                        {row.unverified && <Chip tone="warning">{t("pay.contact.unverified")}</Chip>}
                        {row.reason && <span className="contact-row-note">{row.reason}</span>}
                      </div>
                    ))}
                  </div>
                );
              })}
            </div>}
            {networkHits?.more && prefix !== null && (
              <DashButton variant="plainBlue" size="small" data-testid="contacts-network-more" onClick={() => navigate("contacts", { addContact: { walletId, query: prefix, hit: null }, contact: null, contactPrefill: null })}>
                {t("contacts.network.more")}
              </DashButton>
            )}
            {identity === null ? (
              owner.kind === "starting" ? <p role="status">{t("pay.contact.starting")}</p> : (
                <div data-testid="pay-contact-none">
                  <p>{ownerNote?.next === "join" ? t("pay.contact.joinLine") : ownerNote?.message ?? ownerNote?.title}</p>
                  {owner.kind === "error" && <DashButton variant="plainBlue" size="small" onClick={retryOwner}>{t("pay.contact.retry")}</DashButton>}
                  {owner.kind === "join" && owner.reason === null && <DashButton variant="plainBlue" size="small" data-testid="pay-contact-join" onClick={() => navigate("joinDashPay")}>{t("pay.contact.join")}</DashButton>}
                </div>
              )
            ) : (
              <>
                {(local.isError || (!browsing && remoteReady && remote.isError)) && (
                  <p role="status" data-testid={local.isError ? "pay-picker-error" : "contacts-network-error"}>
                    {t("pay.contact.searchFailed")}{" "}
                    <DashButton size="small" variant="plainBlue" onClick={() => void (local.isError ? local.refetch() : remote.refetch())}>{t("pay.contact.retry")}</DashButton>
                  </p>
                )}
                {!listing || (!browsing && !remoteReady) ? <p role="status">{t("pay.recipient.checking")}</p> : (
                  !local.isError && !remote.isError && optionRows.length === 0 && (
                    <p role="status" data-testid={browsing ? "pay-picker-empty" : "pay-picker-none"}>
                      {t(browsing ? "pay.contact.noContacts" : prefix === null ? "pay.contact.keepTyping" : "pay.contact.noOneFound")}
                    </p>
                  )
                )}
              </>
            )}
          </div>
        )}
      </div>
      <VisuallyHidden><span role="status" data-testid="pay-search-announce">{announce}</span></VisuallyHidden>
      <div id={`${id}-feedback`} className="pay-recipient-feedback" aria-live="polite" data-testid="pay-scan">
        {error ? <span className="field-error">{error}</span> : shieldedHint ? <span className="field-hint">{t("pay.address.shieldedRoute")}</span> : (
          scan.kind === "reading" || scan.kind === "capturing" ? (
            <span className="pay-scan-hint" data-testid="pay-scan-status">{t(scan.kind === "reading" ? "pay.scan.reading" : "pay.scan.capturing")}</span>
          ) : scan.kind === "failed" ? (
            <span className="field-error" data-testid="pay-scan-error">{scan.text}</span>
          ) : (
            <span className="pay-scan-hint">{check && !address && !found && !disabled && !resolved ? t("pay.recipient.checking") : t("pay.scan.hint")}</span>
          )
        )}
      </div>
    </div>
  );
}
