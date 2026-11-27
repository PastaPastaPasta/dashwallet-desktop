// The renderer half of the shell (U-01): deep links and the native menus' commands. dw-app
// routes a link (shell/links.rs); this opens its route, or says why it did not. The native
// menus carry the shortcuts; in a browser (the fixture harness) `browserShortcuts` binds the
// same keys.

import { useEffect, useEffectEvent } from "react";
import type { Backend } from "../api/backend";
import { errorMessage } from "../api/backend";
import type { ConnectRejected } from "../api/generated/ConnectRejected";
import type { LinkRoute } from "../api/generated/LinkRoute";
import type { ShellCommand } from "../api/generated/ShellCommand";
import type { UriRejected } from "../api/generated/UriRejected";
import { inTauri, tauriTools } from "../api/tauri";
import { messageTable, t } from "../i18n";
import { showShellToast } from "../components/toast";
import { NetworkText } from "../network/choice";
import { setLinkDrop } from "./dropGuard";
import { noteLiveLink, SavedLinksText } from "./savedLinks";
import { navigate, openPayForm, ui } from "./ui";

const PAYMENT_REJECTED: Readonly<Record<UriRejected["kind"], string>> = messageTable("app.link.payment", ["doubleSlash", "notDashUri", "unparsable", "bip70Unsupported", "invalidAddress", "invalidAmount"] as const);

const CONNECT_REJECTED: Readonly<Record<ConnectRejected, string>> = {
  get invalid() {
    return t("dashpay.connect.err.invalid");
  },
  get unsupported() {
    return t("dashpay.connect.err.unsupported");
  },
  get needsNewerWallet() {
    return t("dashpay.connect.err.newer");
  },
  get busy() {
    return t("dashpay.connect.err.busy");
  },
};

/** Why a link was not opened, as the window says it; `null` when it opens. */
export function linkProblem(route: LinkRoute): string | null {
  switch (route.kind) {
    case "unsupported":
      switch (route.what) {
        case "addressRequest":
          return t("app.link.addressRequest");
        case "dashConnect":
          return t("app.link.dashConnect");
        case "integration":
          return t("app.link.integration");
        case "paymentRequest":
          return t("app.link.paymentRequest");
      }
      break;
    case "rejected":
      switch (route.reason.kind) {
        case "notALink":
          return t("app.link.notALink");
        case "tooLong":
          return t("app.link.tooLong");
        case "payment":
          return PAYMENT_REJECTED[route.reason.reason.kind];
        case "notSaved":
          return t("app.link.notSaved");
        case "connect":
          return CONNECT_REJECTED[route.reason.reason];
      }
  }
  return null;
}

/** The route's name in the self-test's link trace (never the link's contents: an invitation's
 * stash id stays out too). */
export function traceName(route: LinkRoute): string {
  switch (route.kind) {
    case "unsupported":
      return `unsupported ${route.what}`;
    case "rejected":
      if (route.reason.kind === "payment") return `rejected payment ${route.reason.reason.kind}`;
      return route.reason.kind === "connect" ? `rejected connect ${route.reason.reason}` : `rejected ${route.reason.kind}`;
    default:
      return route.kind;
  }
}

/** Drops the pending `focusPage` wait, if any. */
let cancelLanding = () => {};

/** Moves focus to the page a link or a menu opened (its title, else the page), so keyboard and
 * screen reader users land on it. Sidebar clicks keep their focus. The move waits a frame for
 * the page, and a frame can come late: focus that moved after the user pressed a key or a pointer
 * meanwhile stays (BL-165). A closing sheet's return to its opener, or a key that moved nothing (a
 * modifier, Escape), is not the user moving focus: the page takes it. A newer call replaces a
 * pending one, so a window whose frames are paused (hidden) holds one wait, not one per page. */
export function focusPage() {
  cancelLanding();
  // Where focus was at the first key or pointer press since the call; `undefined` before one.
  let atInput: Element | null | undefined;
  let waiting = true;
  const act = () => void (atInput ??= document.activeElement);
  const stop = () => {
    waiting = false;
    cancelLanding = () => {};
    document.removeEventListener("keydown", act, true);
    document.removeEventListener("pointerdown", act, true);
  };
  document.addEventListener("keydown", act, true);
  document.addEventListener("pointerdown", act, true);
  const frame = requestAnimationFrame(() => {
    if (!waiting) return; // replaced by a newer call, its frame not cancelled in time
    stop();
    const active = document.activeElement;
    if (atInput !== undefined && active && active !== atInput && active !== document.body) return;
    const page = document.querySelector<HTMLElement>(".app-body > main");
    const target = page?.querySelector<HTMLElement>("h1") ?? page;
    if (!target) return;
    if (!target.hasAttribute("tabindex")) target.tabIndex = -1;
    target.focus({ preventScroll: true });
  });
  cancelLanding = () => {
    cancelAnimationFrame(frame);
    stop();
  };
}

/** Said when a link arrives while the wipe sheet is open (D38). */
export const wipeOpenText = () => t("app.link.whileWiping");

/** Opens what `route` leads to. Links open while the vault is locked too: the page is there
 * once it unlocks. While the wipe sheet is open nothing opens: the page would go from under it,
 * and a wipe that then failed would say so nowhere (D38). */
export function applyLink(route: LinkRoute) {
  if (ui.get().wiping) return showShellToast(wipeOpenText());
  noteLiveLink();
  ui.set({ dialog: null });
  switch (route.kind) {
    case "pay": {
      const { uri, username } = route;
      openPayForm({ payPrefill: { address: uri.address, amount: uri.amount, label: uri.label, message: uri.message, username } });
      break;
    }
    case "scanQr":
      openPayForm();
      break;
    case "addContact":
      navigate("contacts", { contactPrefill: { identity: route.identity, username: route.username } });
      break;
    // Kept in the link stash, offered once a wallet exists (DP5-01 D11).
    case "saved":
      showShellToast(SavedLinksText.saved);
      return;
    case "invitation":
      // Saved in the stash either way; during onboarding there is no Join DashPay to show.
      if (ui.get().onboarding) {
        showShellToast(SavedLinksText.invitationSaved);
        return;
      }
      navigate("joinDashPay", { invitation: { linkId: route.linkId, entry: "join" } });
      break;
    // A sheet over the page, not a page: it opens once a wallet is open and unlocked (App).
    case "appConnect": {
      const request = { linkId: route.linkId, returnsTo: route.returnsTo };
      const { appConnect, appConnectQueue } = ui.get();
      if (appConnect === null) ui.set({ appConnect: request, detail: null });
      else ui.set({ appConnectQueue: [...appConnectQueue, request] });
      return;
    }
    default:
      showShellToast(linkProblem(route)!);
      return;
  }
  focusPage();
}

/** Routes a typed or dropped link (File ▸ Open URI…, a drop on the window, QT-019) as a clicked one
 * is, and opens it: why it did not open, or `null` when it did. A refusal of the call is said too. */
export async function openLinkText(backend: Pick<Backend, "routeLink">, network: string, text: string): Promise<string | null> {
  try {
    const route = await backend.routeLink(network, text);
    const why = linkProblem(route);
    if (!why) applyLink(route);
    return why;
  } catch (e) {
    return errorMessage(e);
  }
}

/** Carries out a menu or tray command. A route command closes an open dialog first, as a link
 * does, so the page it opens takes the focus. */
export function runCommand(command: ShellCommand, lock: () => void) {
  // A network switch runs: the page it would act on is about to go (row 74).
  if (ui.get().switching) return;
  // The wipe sheet is open: only lock and discreet mode leave it in place (D38).
  if (ui.get().wiping && command.kind !== "lock" && command.kind !== "toggleDiscreet") return;
  switch (command.kind) {
    case "navigate":
      navigate(command.route, { dialog: null });
      focusPage();
      return;
    case "tools":
      navigate("tools", { toolsTab: command.tab, dialog: null });
      focusPage();
      return;
    case "openUri":
      ui.set({ dialog: "openUri" });
      return;
    case "about":
      ui.set({ dialog: "about" });
      return;
    case "lock":
      lock();
      return;
    case "toggleDiscreet":
      ui.set({ discreet: !ui.get().discreet });
      return;
    // Both dropped while locked (App), as the other dialogs are.
    case "addWallet":
      ui.set({ onboarding: true, dialog: null, detail: null });
      return;
    case "recoveryPhrase":
      ui.set({ dialog: "recoveryPhrase", recoveryWallet: null });
  }
}

/** The menus' shortcuts (shell/menu.rs ITEMS), for a browser, where there is no menu bar. */
const SHORTCUTS: Record<string, ShellCommand> = {
  "1": { kind: "navigate", route: "home" },
  "2": { kind: "navigate", route: "pay" },
  "3": { kind: "navigate", route: "receive" },
  "4": { kind: "navigate", route: "contacts" },
  "5": { kind: "navigate", route: "activity" },
  "6": { kind: "navigate", route: "coinJoin" },
  o: { kind: "openUri" },
  l: { kind: "lock" },
  ",": { kind: "navigate", route: "settings" },
  "shift+d": { kind: "toggleDiscreet" },
  "shift+i": { kind: "tools", tab: "information" },
  "shift+c": { kind: "tools", tab: "console" },
  "shift+p": { kind: "tools", tab: "peers" },
  "shift+r": { kind: "tools", tab: "repair" },
};

export function shortcutFor(e: Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "shiftKey" | "altKey" | "key">): ShellCommand | null {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return null;
  const key = e.key.toLowerCase();
  return SHORTCUTS[e.shiftKey ? `shift+${key}` : key] ?? null;
}

/** One chain of link drains for the page's life (not per effect run, which a network change or
 * a remount restarts): each starts once the batch before it is applied, so batches apply in the
 * order they were drained, and a slow reply cannot land after a later one. */
let drains = Promise.resolve();

/** Once the link takes asked for so far have answered: a page that boots again after a switch
 * waits for them, so a batch taken just before it is kept (below), not lost with the page. */
export function linksTaken(): Promise<void> {
  return drains;
}

/** Links taken just before a network switch, whose reply came while it ran (row 74): kept until
 * the switch is decided. Kept in the tab's session storage, as the page boots again after a
 * switch, and here when there is none. */
const HELD_LINKS = "dw-links-held-by-switch";
type HeldLinks = { network: string; routes: LinkRoute[] };
let heldHere: HeldLinks | null = null;

function holdLinks(network: string, routes: LinkRoute[]) {
  const held = takeHeldLinks();
  if (held && held.network !== network && held.routes.length > 0) showShellToast(NetworkText.linksDropped(held.network));
  heldHere = { network, routes: [...(held?.network === network ? held.routes : []), ...routes] };
  try {
    sessionStorage.setItem(HELD_LINKS, JSON.stringify(heldHere));
  } catch {
    // No storage: they wait in this page only.
  }
}

function takeHeldLinks(): HeldLinks | null {
  let held = heldHere;
  heldHere = null;
  try {
    const saved = sessionStorage.getItem(HELD_LINKS);
    sessionStorage.removeItem(HELD_LINKS);
    if (saved !== null) held = JSON.parse(saved) as HeldLinks;
  } catch {
    // No storage, or not ours: the page's own, if any.
  }
  return held;
}

/** Takes the queued links now and whenever the shell says more wait, one take at a time, and
 * runs the menus' commands. With `--selftest-link-trace` each route is reported, and `ready` once the first
 * take has run. */
export function useShellBridge(backend: Backend, network: string, trace: boolean, lock: () => void) {
  const lockNow = useEffectEvent(lock);
  useEffect(() => {
    const report = (line: string) => {
      if (trace) tauriTools.selftestLinkRouted(line).catch(() => {});
    };
    const apply = (routes: LinkRoute[]) => {
      for (const route of routes) {
        applyLink(route);
        report(traceName(route));
      }
    };
    const drain = async () => {
      // Left queued while a network switch runs: the page that boots next takes them, or this
      // one once the switch is refused (below).
      if (ui.get().switching) return;
      try {
        // Taken before a switch that was refused: this network's, opened now. After a switch,
        // they were routed for the network the window left, so they are not opened on this one.
        const held = takeHeldLinks();
        if (held?.network === network) apply(held.routes);
        else if (held && held.routes.length > 0) showShellToast(NetworkText.linksDropped(held.network));
        // Applied even after an unmount: the queue is drained, so these routes exist only here.
        const routes = await backend.takeLinks(network);
        // A switch began while they were taken: kept until it is decided.
        if (ui.get().switching) {
          holdLinks(network, routes);
          return;
        }
        apply(routes);
      } catch (e) {
        showShellToast(t("app.link.failed", { error: errorMessage(e) }));
      }
    };
    const take = () => (drains = drains.then(drain));
    const unlisten = backend.onShellEvent((event) => {
      if (event.kind === "linksPending") void take();
      else runCommand(event.command, lockNow);
    });
    // The first take once the listener is in: a link queued in between still gets heard.
    void unlisten
      .then(take, take)
      .then(() => report("ready"));
    const onKey = (e: KeyboardEvent) => {
      const command = shortcutFor(e);
      if (!command) return;
      e.preventDefault();
      runCommand(command, lockNow);
    };
    if (!inTauri()) window.addEventListener("keydown", onKey);
    // A payment link dropped on the window opens as Open URI… would; what stops it is said in a
    // toast. Not during setup or a wipe (routing would save it in the stash) or a switch. The
    // newest drop wins: an older one's late reply, or one after the page went, is dropped.
    let dropped = 0;
    let live = true;
    setLinkDrop((text) => {
      const { switching, wiping, onboarding } = ui.get();
      if (switching || wiping || onboarding) return;
      const mine = ++dropped;
      void backend.routeLink(network, text).then(
        (route) => {
          if (!live || mine !== dropped || ui.get().switching || ui.get().wiping) return;
          const why = linkProblem(route);
          if (why) showShellToast(why);
          else applyLink(route);
        },
        (e) => live && mine === dropped && showShellToast(errorMessage(e)),
      );
    });
    let switching = ui.get().switching;
    const stopWatching = ui.subscribe(() => {
      const now = ui.get().switching;
      if (switching && !now) void take();
      switching = now;
    });
    return () => {
      void unlisten.then((f) => f());
      window.removeEventListener("keydown", onKey);
      setLinkDrop(null);
      live = false;
      stopWatching();
    };
  }, [backend, network, trace]);
}
