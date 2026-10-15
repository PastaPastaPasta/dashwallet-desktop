// Tools ▸ Address Book (QT-095…098): the Swift `AddressBookViewModel`, ported. Sending and
// receiving lists sorted by label, dash-qt's wildcard search (U-05 D13), new and edit with
// dash-qt's messages, delete for sending entries only (D14), the CSV export and a QR of an entry.
// In selection mode (the pickers in Pay and Sign, QT-097) choosing an entry hands it back.

import { hasCode, isAppError, type Backend } from "../api/backend";
import type { AddressBookEntry } from "../api/generated/AddressBookEntry";
import type { AddressPurposeName } from "../api/generated/AddressPurposeName";
import { failureText, Model } from "../onboarding/model";

export const AddressBookText = {
  sendingHeader: "These are your Dash addresses for sending payments. Always check the amount and the receiving address before sending coins.",
  receivingHeader:
    "These are your Dash addresses for receiving payments. Use the 'Create new receiving address' button in the receive tab to create new addresses.",
  searchPlaceholder: "Enter address or label to search",
  noLabel: "(no label)",
  newSending: "New sending address",
  editSending: "Edit sending address",
  editReceiving: "Edit receiving address",
  receivingNotDeletable: "Receiving addresses cannot be deleted.",
  entryNotFound: "This address is not in the address book.",
  invalidAddress: (a: string) => `The entered address "${a}" is not a valid Dash address.`,
  existsAsReceiving: (a: string, label: string) =>
    `Address "${a}" already exists as a receiving address with label "${label}" and so cannot be added as a sending address.`,
  alreadyInBook: (a: string, label: string) => `The entered address "${a}" is already in the address book with label "${label}".`,
};

/** dash-qt's wildcard filter: `*` any run, `?` one character, case-insensitive, anywhere in the
 * text. A greedy matcher that backs up only to the last star, so the work stays within pattern ×
 * text however many stars there are (a regular expression backtracks exponentially, U-05 D39). */
export function wildcard(pattern: string): (text: string) => boolean {
  // "Anywhere in the text" is a star at each end; a run of stars is one star.
  // Each character folded alone, as the regular expression's `i` did: lowercasing a whole string
  // depends on context (a final sigma) and can turn one character into two (İ).
  const fold = (text: string) => [...text].map((c) => c.toLowerCase());
  const p = fold(`*${pattern}*`.replace(/\*+/g, "*"));
  return (text) => {
    const t = fold(text);
    let i = 0;
    let j = 0;
    let star = -1;
    let resume = 0;
    while (j < t.length) {
      if (i < p.length && (p[i] === "?" || p[i] === t[j])) {
        i += 1;
        j += 1;
      } else if (i < p.length && p[i] === "*") {
        star = i;
        i += 1;
        resume = j;
      } else if (star >= 0) {
        i = star + 1;
        resume += 1;
        j = resume;
      } else {
        return false;
      }
    }
    while (p[i] === "*") i += 1;
    return i === p.length;
  };
}

const quote = (s: string) => `"${s.replaceAll('"', '""')}"`;

/** dash-qt's export: a header and one row per entry, every field quoted. */
export function addressBookCsv(entries: AddressBookEntry[]): string {
  return [["Label", "Address"], ...entries.map((e) => [e.label, e.address])].map((r) => r.map(quote).join(",") + "\n").join("");
}

const byLabel = (a: AddressBookEntry, b: AddressBookEntry) => a.label.localeCompare(b.label, "en", { sensitivity: "base" }) || a.address.localeCompare(b.address);

type Api = Pick<Backend, "addressBook" | "saveAddressBookEntry" | "deleteAddressBookEntry" | "classifyAddress">;

export class AddressBookModel extends Model {
  purpose: AddressPurposeName;
  search = "";
  /** Both lists, for the duplicate messages; `null` until loaded. */
  all: AddressBookEntry[] | null = null;
  error: string | null = null;
  /** The entry the QR shows. */
  qrEntry: AddressBookEntry | null = null;
  /** What the picker chose. */
  chosen: AddressBookEntry | null = null;

  constructor(
    private readonly api: Api,
    readonly walletId: string,
    /** The network's directory name, for the address check. */
    private readonly network: string,
    purpose: AddressPurposeName = "send",
    readonly selectionMode = false,
  ) {
    super();
    this.purpose = purpose;
  }

  get header(): string {
    return this.purpose === "send" ? AddressBookText.sendingHeader : AddressBookText.receivingHeader;
  }

  /** dash-qt: New and Delete for sending addresses only. */
  get canCreate(): boolean {
    return this.purpose === "send";
  }

  get canDelete(): boolean {
    return this.purpose === "send";
  }

  /** The current list, filtered and sorted by label. */
  get entries(): AddressBookEntry[] {
    const match = wildcard(this.search);
    return (this.all ?? []).filter((e) => e.purpose === this.purpose && (this.search === "" || match(e.label) || match(e.address))).sort(byLabel);
  }

  labelText(e: AddressBookEntry): string {
    return e.label || AddressBookText.noLabel;
  }

  async load() {
    try {
      const [send, receive] = await Promise.all([this.api.addressBook(this.walletId, "send"), this.api.addressBook(this.walletId, "receive")]);
      this.all = [...send, ...receive];
      this.error = null;
    } catch (e) {
      this.error = failureText(e);
    }
    this.changed();
  }

  setPurpose(purpose: AddressPurposeName) {
    this.purpose = purpose;
    this.qrEntry = null;
    this.changed();
  }

  setSearch(search: string) {
    this.search = search;
    this.changed();
  }

  /** New (`replace: false`) or Edit (`replace: true`, the label only). `true` once saved. */
  async save(address: string, label: string, replace: boolean): Promise<boolean> {
    const a = address.trim();
    const known = (this.all ?? []).find((e) => e.address === a);
    if (!replace) {
      const kind = await this.api.classifyAddress(this.network, a).catch(() => null);
      if (kind?.kind !== "core") return this.#fail(AddressBookText.invalidAddress(a));
      if (known) return this.#fail(known.purpose === "receive" ? AddressBookText.existsAsReceiving(a, known.label) : AddressBookText.alreadyInBook(a, known.label));
    }
    const purpose = replace ? (known?.purpose ?? this.purpose) : "send";
    try {
      await this.api.saveAddressBookEntry(this.walletId, a, label.trim(), purpose, replace);
    } catch (e) {
      // The engine knows an entry this list did not have yet (changed since the last read):
      // dash-qt's text names its label.
      const entry = known ?? (hasCode(e, "labels.duplicate_address") || hasCode(e, "labels.own_address") ? await this.#entryOf(a) : undefined);
      return this.#fail(this.#errorText(e, a, entry));
    }
    this.error = null;
    await this.load();
    return true;
  }

  async delete(address: string) {
    const known = this.all?.find((e) => e.address === address);
    if (known?.purpose === "receive") return void this.#fail(AddressBookText.receivingNotDeletable);
    try {
      await this.api.deleteAddressBookEntry(this.walletId, address);
      this.error = null;
      if (this.qrEntry?.address === address) this.qrEntry = null;
    } catch (e) {
      this.#fail(this.#errorText(e, address, known));
    }
    await this.load();
  }

  /** The sending list as dash-qt exports it. */
  exportCsv(): string {
    return addressBookCsv(this.entries);
  }

  /** The editor opens or closes: a message of the last attempt goes. */
  clearError() {
    this.error = null;
    this.changed();
  }

  showQr(e: AddressBookEntry | null) {
    this.qrEntry = e;
    this.changed();
  }

  /** In selection mode, picks `e` (QT-097); otherwise nothing. */
  choose(e: AddressBookEntry) {
    if (!this.selectionMode) return;
    this.chosen = e;
    this.changed();
  }

  #errorText(e: unknown, address: string, known: AddressBookEntry | undefined): string {
    if (!isAppError(e)) return failureText(e);
    switch (e.code) {
      case "labels.invalid_address":
        return AddressBookText.invalidAddress(address);
      case "labels.duplicate_address":
        return AddressBookText.alreadyInBook(address, known?.label ?? "");
      case "labels.own_address":
        return AddressBookText.existsAsReceiving(address, known?.label ?? "");
      case "labels.entry_not_found":
        return AddressBookText.entryNotFound;
      case "labels.receive_entry_not_deletable":
        return AddressBookText.receivingNotDeletable;
      default:
        return failureText(e);
    }
  }

  async #entryOf(address: string): Promise<AddressBookEntry | undefined> {
    for (const purpose of ["receive", "send"] as const) {
      const list = await this.api.addressBook(this.walletId, purpose).catch(() => []);
      const found = list.find((e) => e.address === address);
      if (found) return found;
    }
    return undefined;
  }

  #fail(text: string): false {
    this.error = text;
    this.changed();
    return false;
  }
}
