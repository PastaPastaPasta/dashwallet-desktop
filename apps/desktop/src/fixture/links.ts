// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// `route_link` for the fixture: the shapes dw-app's router returns, for the links the tests
// type into Open URI. dw-app's shell/links.rs is the router (dash-qt's URI handler, the iOS
// link parsers) and is tested there; this only tells the cases apart well enough to drive
// every route and every message.

import type { LinkRoute } from "../api/generated/LinkRoute";
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
      if (/^\/\/invite\?/i.test(rest)) return { kind: "invitation" };
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
