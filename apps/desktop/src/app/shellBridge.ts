// The renderer half of the shell (U-01): deep links and the native menus' commands. dw-app
// routes a link (shell/links.rs); this opens its route, or says why it did not. The native
// menus carry the shortcuts; in a browser (the fixture harness) `browserShortcuts` binds the
// same keys.

import { useEffect, useEffectEvent } from "react";
import type { Backend } from "../api/backend";
import { errorMessage } from "../api/backend";
import type { LinkRoute } from "../api/generated/LinkRoute";
import type { ShellCommand } from "../api/generated/ShellCommand";
import type { UriRejected } from "../api/generated/UriRejected";
import { inTauri, tauriTools } from "../api/tauri";
import { showToast } from "../components/toast";
import { navigate, ui } from "./ui";

const PAYMENT_REJECTED: Record<UriRejected["kind"], string> = {
  doubleSlash: "This payment link starts with dash:// — Dash links start with dash: alone",
  notDashUri: "This is not a Dash payment link",
  unparsable: "This payment link could not be read",
  bip70Unsupported: "Payment requests (BIP70) are not supported",
  invalidAddress: "The address in this payment link is not valid on this network",
  invalidAmount: "The amount in this payment link is not valid",
};

/** Why a link was not opened, as the window says it; `null` when it opens. */
export function linkProblem(route: LinkRoute): string | null {
  switch (route.kind) {
    case "unsupported":
      switch (route.what) {
        case "addressRequest":
          return "Dash Wallet does not hand addresses to other apps";
        case "dashConnect":
          return "DashConnect links are not supported";
        case "integration":
          return "Exchange links are not supported";
        case "paymentRequest":
          return "Payment requests (BIP70) are not supported";
      }
      break;
    case "rejected":
      switch (route.reason.kind) {
        case "notALink":
          return "This is not a link Dash Wallet can open";
        case "tooLong":
          return "This link is too long to open";
        case "payment":
          return PAYMENT_REJECTED[route.reason.reason.kind];
      }
  }
  return null;
}

/** The route's name in the self-test's link trace (never the link's contents). */
export function traceName(route: LinkRoute): string {
  switch (route.kind) {
    case "unsupported":
      return `unsupported ${route.what}`;
    case "rejected":
      return route.reason.kind === "payment" ? `rejected payment ${route.reason.reason.kind}` : `rejected ${route.reason.kind}`;
    default:
      return route.kind;
  }
}

/** Moves focus to the page a link or a menu opened (its title, else the page), so keyboard and
 * screen reader users land on it. Sidebar clicks keep their focus. */
function focusPage() {
  requestAnimationFrame(() => {
    const page = document.querySelector<HTMLElement>(".app-body > main");
    const target = page?.querySelector<HTMLElement>("h1") ?? page;
    if (!target) return;
    if (!target.hasAttribute("tabindex")) target.tabIndex = -1;
    target.focus({ preventScroll: true });
  });
}

/** Opens what `route` leads to. Links open while the vault is locked too: the page is there
 * once it unlocks. */
export function applyLink(route: LinkRoute) {
  ui.set({ dialog: null });
  switch (route.kind) {
    case "pay": {
      const { uri, username } = route;
      navigate("pay", {
        payTab: "address",
        payPrefill: { address: uri.address, amount: uri.amount, label: uri.label, message: uri.message, username },
        payLinks: ui.get().payLinks + 1,
      });
      break;
    }
    case "scanQr":
      navigate("pay", { payTab: "address", payPrefill: null, payLinks: ui.get().payLinks + 1 });
      break;
    case "addContact":
      navigate("contacts", { contactPrefill: { identity: route.identity, username: route.username } });
      break;
    case "invitation":
      navigate("joinDashPay", { invitation: true });
      break;
    default:
      showToast(linkProblem(route)!);
      return;
  }
  focusPage();
}

/** Carries out a menu or tray command. A route command closes an open dialog first, as a link
 * does, so the page it opens takes the focus. */
export function runCommand(command: ShellCommand, lock: () => void) {
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
      ui.set({ dialog: "recoveryPhrase" });
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

/** Takes the queued links now and whenever the shell says more wait, one take at a time, and
 * runs the menus' commands. With `--selftest-link-trace` each route is reported, and `ready` once the first
 * take has run. */
export function useShellBridge(backend: Backend, network: string, trace: boolean, lock: () => void) {
  const lockNow = useEffectEvent(lock);
  useEffect(() => {
    const report = (line: string) => {
      if (trace) tauriTools.selftestLinkRouted(line).catch(() => {});
    };
    const drain = async () => {
      try {
        // Applied even after an unmount: the queue is drained, so these routes exist only here.
        const routes = await backend.takeLinks(network);
        for (const route of routes) {
          applyLink(route);
          report(traceName(route));
        }
      } catch (e) {
        showToast(`Could not open the link: ${errorMessage(e)}`);
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
    return () => {
      void unlisten.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  }, [backend, network, trace]);
}
