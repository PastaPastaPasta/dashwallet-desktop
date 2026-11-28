import type { Backend } from "../../api/backend";
import type { ContactList } from "../../api/generated/ContactList";
import type { PaymentUri } from "../../api/generated/PaymentUri";
import type { UserHit } from "../../api/generated/UserHit";
import { linkProblem } from "../../app/shellBridge";
import { t } from "../../i18n";
import { pickerGroups, pickerHits, type PickerGroup, type PickerRow } from "./contact";
import { addressProblem } from "./model";

export type RecipientApi = Pick<Backend, "classifyAddress" | "routeLink" | "dashpayVerifyScanned">;

export type RecipientInput = {
  kind: "empty" | "name" | "address" | "paymentLink" | "userLink" | "invalidLink";
  text: string;
};

export type AddressRecipient = PaymentUri & {
  kind: "address";
  route: "core" | "platform" | "shielded";
  username: string | null;
};

export type FoundHit = Omit<UserHit, "avatarUrl" | "username"> & { username: string | null };

export type RecipientSuggestion = { kind: "contact"; payee: PickerRow; group: PickerGroup["id"] } | { kind: "user"; hit: FoundHit };
export type ResolvedInput = AddressRecipient | { kind: "user"; hit: FoundHit } | { kind: "invalid"; error: string };

export function parseRecipientInput(value: string): RecipientInput {
  const text = value.trim();
  if (!text) return { kind: "empty", text };
  if (/^dashpay:\/\/user\?/i.test(text)) return { kind: "userLink", text };
  if (/^(?:dash:|pay:|dashwallet:\/\/pay=)/i.test(text)) return { kind: "paymentLink", text };
  if (/^[a-z][a-z0-9+.-]*:/i.test(text)) return { kind: "invalidLink", text };

  // Long address-like strings go through the engine's address classifier, including a bad
  // checksum or prefix. Short usernames and names (with spaces) go to Contacts first.
  if (/^[a-z0-9]{25,}$/i.test(text)) return { kind: "address", text };
  return { kind: "name", text };
}

export const directRecipient = ({ kind }: RecipientInput) => kind !== "empty" && kind !== "name";

export async function resolveRecipient(api: RecipientApi, walletId: string, network: string, input: RecipientInput): Promise<ResolvedInput> {
  const { text, kind } = input;
  const invalid = (error = t("pay.error.invalidAddress")): ResolvedInput => ({ kind: "invalid", error });
  if (kind === "invalidLink" || kind === "empty" || kind === "name") return invalid();
  if (kind === "userLink") {
    const hit = await api.dashpayVerifyScanned(walletId, text);
    return { kind: "user", hit };
  }
  let uri: PaymentUri = { address: text, amount: null, label: null, message: null };
  let username: string | null = null;
  if (kind === "paymentLink") {
    const link = await api.routeLink(network, text);
    if (link.kind !== "pay") return invalid(linkProblem(link) ?? undefined);
    ({ uri, username } = link);
  }
  const classified = await api.classifyAddress(network, uri.address);
  if (classified.kind === "invalid") return invalid(addressProblem(classified) ?? undefined);
  return { kind: "address", ...uri, route: classified.kind, username };
}

export function recipientSuggestions(list: ContactList, hits: UserHit[] | undefined): RecipientSuggestion[] {
  return [
    ...pickerGroups(list).flatMap((group) => group.rows.map((payee) => ({ kind: "contact" as const, payee, group: group.id }))),
    ...pickerHits(hits, list).shown.map((hit) => ({ kind: "user" as const, hit })),
  ];
}

export type RecipientEntry = { text: string; address: AddressRecipient | null; revision: number };

export function editRecipient(entry: RecipientEntry, text: string): RecipientEntry {
  return { text, address: null, revision: entry.revision + 1 };
}
