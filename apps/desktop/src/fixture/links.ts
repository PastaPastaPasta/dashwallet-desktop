// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// `route_link` for the fixture: the shapes dw-app's router returns, for the links the tests
// type into Open URI. dw-app's shell/links.rs is the router (dash-qt's URI handler, the iOS
// link parsers) and is tested there; this only tells the cases apart well enough to drive
// every route and every message.

import type { LinkRoute } from "../api/generated/LinkRoute";
import type { StashedLinkInfo } from "../api/generated/StashedLinkInfo";
import type { StashedLinkKindName } from "../api/generated/StashedLinkKindName";
import type { UriRejected } from "../api/generated/UriRejected";

/** A Base58 testnet address (P2PKH `y`, P2SH `8`). */
const TESTNET_ADDRESS = /^[y8][1-9A-HJ-NP-Za-km-z]{25,34}$/;

const rejected = (reason: UriRejected): LinkRoute => ({ kind: "rejected", reason: { kind: "payment", reason } });

function payment(rest: string): LinkRoute {
  if (rest.startsWith("//")) return rejected({ kind: "doubleSlash" });
  const q = rest.search(/[?&]/);
  const address = q < 0 ? rest : rest.slice(0, q);
  const params = new URLSearchParams(q < 0 ? "" : rest.slice(q + 1));
  if ([...params.keys()].some((k) => k.startsWith("req-"))) return rejected({ kind: "unparsable" });
  if (!address && params.has("r")) return { kind: "unsupported", what: "paymentRequest" };
  if (!TESTNET_ADDRESS.test(address)) return rejected({ kind: "invalidAddress", problem: { kind: "invalidBase58Prefix" } });
  let amount: number | null = null;
  const a = params.get("amount");
  if (a !== null) {
    const dash = Number(a);
    if (!/^\d+(\.\d{0,8})?$/.test(a) || !(dash <= 21e6)) return rejected({ kind: "invalidAmount" });
    amount = Math.round(dash * 1e8) || null;
  }
  return {
    kind: "pay",
    uri: { address, amount, label: params.get("label") || null, message: params.get("message") || null },
    username: params.get("user") || null,
  };
}

export function routeLink(text: string): LinkRoute {
  const t = text.trim();
  const colon = t.indexOf(":");
  const scheme = colon < 0 ? "" : t.slice(0, colon).toLowerCase();
  const rest = t.slice(colon + 1);
  switch (scheme) {
    case "dash":
    case "pay":
      return payment(rest);
    case "dashwallet":
      if (/^\/\/scanqr$/i.test(rest)) return { kind: "scanQr" };
      if (rest.startsWith("//request=")) return { kind: "unsupported", what: "addressRequest" };
      break;
    case "dashpay": {
      // Its stash id comes with the intake below.
      if (/^\/\/invite\?/i.test(rest)) return { kind: "invitation", linkId: "" };
      const user = /^\/\/user\?(.*)$/i.exec(rest);
      if (user) {
        const p = new URLSearchParams(user[1]);
        const identity = p.get("id") ?? "";
        const username = (p.get("username") ?? "").replace(/\.dash$/, "");
        if (/^[1-9A-HJ-NP-Za-km-z]{43,44}$/.test(identity) && username) return { kind: "addContact", identity, username };
      }
      break;
    }
  }
  return { kind: "rejected", reason: { kind: "notALink" } };
}

/** A link in the fixture's stash (dw-vault's link stash, DP5-01): what `stashed_links` lists,
 * and the route `open_stashed_link` gives a payment or contact link. */
export interface FixtureStashedLink {
  info: StashedLinkInfo;
  route: LinkRoute;
}

/** dw-engine's TTLs (D8): payment 7 days, the others 30. */
const TTL_DAYS: Record<StashedLinkKindName, number> = { payment: 7, contact: 30, invitation: 30 };
/** dw-vault's MAX_STASHED_LINKS. */
const MAX_STASHED = 16;

const STASH_KIND: Partial<Record<LinkRoute["kind"], StashedLinkKindName>> = {
  pay: "payment",
  addContact: "contact",
  invitation: "invitation",
};
const stashKind = (route: LinkRoute): StashedLinkKindName | null => STASH_KIND[route.kind] ?? null;

/** Puts `route` in `stash` (the same link again keeps its entry; a full stash drops its oldest
 * payment or contact link first) and returns the entry. */
export function stashRoute(stash: FixtureStashedLink[], route: LinkRoute, nowSecs = Math.floor(Date.now() / 1000)): StashedLinkInfo {
  const kind = stashKind(route)!;
  const same = JSON.stringify(route);
  const found = stash.find((s) => s.info.kind === kind && kind !== "invitation" && JSON.stringify(s.route) === same);
  if (found) return found.info;
  if (stash.length >= MAX_STASHED) {
    const drop = stash.findIndex((s) => s.info.kind !== "invitation");
    stash.splice(drop < 0 ? 0 : drop, 1);
  }
  const id = (stash.reduce((n, s) => Math.max(n, parseInt(s.info.id, 16)), 0) + 1).toString(16).padStart(32, "0");
  const info = { id, kind, stashedAt: nowSecs, expiresAt: nowSecs + TTL_DAYS[kind] * 86_400 };
  stash.push({ info, route: kind === "invitation" ? { kind: "invitation", linkId: id } : route });
  return info;
}

/** dw-app's `links::intake` (D2): an invitation always waits in the stash, a payment or
 * contact link while the network has no wallet; the rest route at once. */
export function intake(stash: FixtureStashedLink[], route: LinkRoute, hasWallet: boolean): LinkRoute {
  if (route.kind === "invitation") return { kind: "invitation", linkId: stashRoute(stash, route).id };
  if ((route.kind === "pay" || route.kind === "addContact") && !hasWallet) return { kind: "saved", link: stashRoute(stash, route) };
  return route;
}
