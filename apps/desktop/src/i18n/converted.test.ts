// Keeps converted screens converted (MP-14): a `.tsx` file listed in CONVERTED (a file, or every
// file under a directory) may not show text that bypasses the catalog: JSX text, or a literal in
// an attribute that is read or shown (`title`, `aria-label`, `placeholder`, …). Nor may it build a
// sentence from fragments: a copy button named from a noun, or a message given another message as a
// value, where a translator cannot control case, inflection or word order. Each later slice adds the
// paths it converts.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { parseSync, Visitor } from "vite";
import { describe, expect, it } from "vitest";

/** Paths under src/ whose `.tsx` files take their text from the catalog, by screen area. Each area's
 * pull request fills its own list. */
const AREAS: Record<string, string[]> = {
  shell: ["components/shell.tsx", "components/networkPicker.tsx"],

  lock: ["screens/LockScreen.tsx"],

  settings: ["screens/settings/About.tsx", "screens/settings/CoinJoin.tsx", "screens/settings/General.tsx", "screens/settings/Identities.tsx", "screens/settings/LocalCurrency.tsx", "screens/settings/Privacy.tsx", "screens/settings/Security.tsx", "screens/settings/SettingsPage.tsx", "screens/settings/Updates.tsx", "screens/settings/Wallets.tsx", "screens/settings/parts.tsx"],

  components: ["components/basics.tsx", "components/coins.tsx", "components/copy.tsx", "components/detail.tsx", "components/emptyArt.tsx", "components/history.tsx", "components/passphrase.tsx", "components/phrase.tsx", "components/qr.tsx", "components/sheet.tsx", "components/toast.tsx"],

  app: ["app/App.tsx", "screens/StoreRefusal.tsx", "screens/ShellDialogs.tsx", "screens/Routes.tsx", "screens/SavedLinks.tsx", "fiat/entry.tsx", "fiat/rates.tsx"],

  onboarding: ["screens/Onboarding.tsx", "screens/ForgotPassphrase.tsx", "screens/RecoveryPhrase.tsx", "screens/PreReleaseNotice.tsx", "fiat/consent.tsx"],

  home: ["screens/Overview.tsx", "screens/UnfinishedTransfersCard.tsx", "screens/ProtocolAdvisoryBanner.tsx", "screens/RestoredBanner.tsx", "screens/DroppedNoticeBanner.tsx", "screens/RestoredStatePanel.tsx"],

  activity: ["screens/SourceDetailSheet.tsx"],

  receive: ["screens/Receive.tsx"],

  pay: ["screens/pay"],

  contacts: ["screens/contacts"],

  dashpay: ["screens/dashpay/AboutDashPay.tsx", "screens/dashpay/AppConnectSheet.tsx", "screens/dashpay/Claim.tsx", "screens/dashpay/DashPayCard.tsx", "screens/dashpay/EditProfileSheet.tsx", "screens/dashpay/JoinDashPay.tsx", "screens/dashpay/MyProfile.tsx", "screens/dashpay/Registration.tsx", "screens/dashpay/ShareSheet.tsx", "screens/dashpay/TopUpSheet.tsx", "screens/dashpay/Welcome.tsx", "screens/dashpay/WithdrawSheet.tsx"],

  coinjoin: ["screens/coinjoin"],

  notifications: ["screens/notifications"],

  masternodes: ["screens/masternodes/CreateProposalSheet.tsx", "screens/masternodes/MasternodesPage.tsx", "screens/masternodes/Mine.tsx", "screens/masternodes/MyMasternodes.tsx", "screens/masternodes/NameVoteSheet.tsx", "screens/masternodes/NamesTab.tsx", "screens/masternodes/NumbersSheet.tsx", "screens/masternodes/ProposalPane.tsx", "screens/masternodes/SharedPane.tsx", "screens/masternodes/StatusItem.tsx", "screens/masternodes/VoteSheet.tsx", "screens/masternodes/common.tsx", "screens/masternodes/credential.tsx"],

  tools: ["screens/tools"],

  transfer: ["screens/TransferSheet.tsx", "screens/PlatformMoveSheet.tsx", "screens/PlatformFundSheet.tsx", "screens/ShieldedSheet.tsx"],
};

export const CONVERTED = Object.values(AREAS).flat();

/** Text that may stay literal: `file: [text, …]`. Keep it short and say why. */
const ALLOWED: Record<string, string[]> = {
  // The route header's hidden " to " between two pool names: needs a whole-sentence key.
  "screens/TransferSheet.tsx": ["to"],
  // The brand, the wordmark's name in every language (glossary).
  "components/basics.tsx": ["Dash"],
  // The Open URI field's placeholder: the dash: scheme, typed as is in every language.
  "screens/ShellDialogs.tsx": ["dash:"],
  // The proposal link's placeholder: the https:// scheme, typed as is in every language.
  "screens/masternodes/CreateProposalSheet.tsx": ["https://"],
};

/** Messages that take another message as a value on purpose: `file: [the value's key, …]`. Say why. */
const ALLOWED_FRAGMENTS: Record<string, string[]> = {
  // The rates disclosure is a whole sentence, set before or after another whole sentence.
  "fiat/consent.tsx": ["rates.disclosure"],
  // A choice's note and its vote count, set side by side with "·": no sentence around them.
  "screens/masternodes/NameVoteSheet.tsx": ["masternodes.names.sheet.choiceNote.…"],
};

/** Attributes whose value a user reads or hears. Props of our components that carry copy too. */
const TEXT_ATTRIBUTES = new Set(["title", "aria-label", "aria-description", "aria-placeholder", "aria-roledescription", "placeholder", "alt", "label", "text", "why", "confirm", "message", "submitLabel"]);

/** Functions whose arguments a user reads: a toast's text. */
const TEXT_CALLS = new Set(["showToast"]);

const src = resolve(import.meta.dirname, "..");
const LETTER = /\p{L}/u;

function files(path: string): string[] {
  const full = join(src, path);
  if (!statSync(full).isDirectory()) return [full];
  return readdirSync(full, { recursive: true, encoding: "utf8" })
    .filter((f) => f.endsWith(".tsx"))
    .map((f) => join(full, f));
}

interface Node {
  type: string;
  start: number;
  [k: string]: unknown;
}

/** The hard-coded strings in a `.tsx` source, as `line: text`. */
export function hardCoded(file: string, source: string): string[] {
  const { program, errors } = parseSync(file, source, { lang: "tsx" });
  if (errors.length) throw new Error(`${file}: ${errors[0].message}`);
  const found: { at: number; text: string }[] = [];
  const add = (at: number, text: string) => {
    const t = text.replace(/\s+/g, " ").trim();
    if (LETTER.test(t)) found.push({ at: at + text.search(/\S/), text: t });
  };
  /** String literals an expression can evaluate to (`c ? "a" : "b"`, `x ?? "a"`, `` `a ${b}` ``). */
  const literals = (e: Node | null | undefined) => {
    if (!e) return;
    if (e.type === "Literal" && typeof e.value === "string") add(e.start, e.value);
    else if (e.type === "TemplateLiteral") {
      for (const q of e.quasis as { start: number; value: { cooked: string } }[]) add(q.start, q.value.cooked);
      for (const expression of e.expressions as Node[]) literals(expression);
    }
    else if (e.type === "ConditionalExpression") {
      literals(e.consequent as Node);
      literals(e.alternate as Node);
    } else if (e.type === "LogicalExpression" || (e.type === "BinaryExpression" && e.operator === "+")) {
      literals(e.left as Node);
      literals(e.right as Node);
    }
  };
  const children = (nodes: Node[]) => {
    for (const c of nodes) if (c.type === "JSXExpressionContainer") literals(c.expression as Node);
  };
  new Visitor({
    CallExpression: (n) => {
      const callee = n.callee as unknown as Node;
      if (callee.type === "Identifier" && TEXT_CALLS.has(callee.name as string)) for (const a of n.arguments as unknown as Node[]) literals(a);
    },
    JSXText: (n) => add(n.start, n.value),
    JSXElement: (n) => children(n.children as unknown as Node[]),
    JSXFragment: (n) => children(n.children as unknown as Node[]),
    JSXAttribute: (n) => {
      const name = n.name.type === "JSXIdentifier" ? n.name.name : `${n.name.namespace.name}:${n.name.name.name}`;
      if (!TEXT_ATTRIBUTES.has(name) || !n.value) return;
      const v = n.value as unknown as Node;
      if (v.type === "Literal") literals(v);
      else if (v.type === "JSXExpressionContainer") literals(v.expression as Node);
    },
  }).visit(program);
  const line = (at: number) => source.slice(0, at).split("\n").length;
  return found.sort((a, b) => a.at - b.at).map((f) => `${line(f.at)}: ${f.text}`);
}

const MESSAGE_CALLS = new Set(["t", "tr", "tNow"]);

/** A call of `t` (or `tr`, `tNow`, `t.rich`) with a literal key, and that key. */
function messageKey(e: Node | null | undefined): string | null {
  if (e?.type !== "CallExpression") return null;
  const callee = e.callee as Node;
  const named = callee.type === "Identifier" ? MESSAGE_CALLS.has(callee.name as string) : callee.type === "MemberExpression" && (callee.object as Node).type === "Identifier" && MESSAGE_CALLS.has((callee.object as Node).name as string);
  const [key] = e.arguments as Node[];
  return named ? keyText(key) : null;
}

/** A message key as written: a literal, a template (`a.${k}` reads "a.…"), or either side of `?:`. */
function keyText(key: Node | undefined): string | null {
  if (key?.type === "Literal" && typeof key.value === "string") return key.value;
  if (key?.type === "TemplateLiteral") return (key.quasis as { value: { cooked: string } }[]).map((q) => q.value.cooked).join("…");
  if (key?.type !== "ConditionalExpression") return null;
  const [a, b] = [keyText(key.consequent as Node), keyText(key.alternate as Node)];
  return a !== null && b !== null ? `${a} | ${b}` : null;
}

/** The sentences a `.tsx` source builds from fragments, as `line: what`: a CopyValue named from a
 * noun (`label=`, read in "Copy {label}"), and a message given another message as a value. */
export function fragments(file: string, source: string): string[] {
  const { program, errors } = parseSync(file, source, { lang: "tsx" });
  if (errors.length) throw new Error(`${file}: ${errors[0].message}`);
  const found: { at: number; text: string }[] = [];
  new Visitor({
    JSXOpeningElement: (n) => {
      const name = n.name as unknown as Node;
      if (name.type !== "JSXIdentifier" || name.name !== "CopyValue") return;
      for (const a of n.attributes as unknown as Node[])
        if (a.type === "JSXAttribute" && (a.name as Node).name === "label") found.push({ at: a.start, text: "CopyValue label=" });
    },
    CallExpression: (n) => {
      const outer = messageKey(n as unknown as Node);
      const values = (n.arguments as unknown as Node[])[1];
      if (outer === null || values?.type !== "ObjectExpression") return;
      for (const p of values.properties as Node[]) {
        const inner = p.type === "Property" ? messageKey(p.value as Node) : null;
        if (inner !== null) found.push({ at: p.start, text: `${outer} takes ${inner}` });
      }
    },
  }).visit(program);
  const line = (at: number) => source.slice(0, at).split("\n").length;
  return found.sort((a, b) => a.at - b.at).map((f) => `${line(f.at)}: ${f.text}`);
}

describe("converted screens", () => {
  for (const path of CONVERTED)
    for (const file of files(path)) {
      const name = relative(src, file);
      it(`${name} has no hard-coded user-visible text`, () => {
        const allowed = new Set(ALLOWED[name] ?? []);
        const found = hardCoded(file, readFileSync(file, "utf8")).filter((f) => !allowed.has(f.slice(f.indexOf(": ") + 2)));
        expect(found).toEqual([]);
      });
      it(`${name} builds no sentence from fragments`, () => {
        const allowed = ALLOWED_FRAGMENTS[name] ?? [];
        expect(fragments(file, readFileSync(file, "utf8")).filter((f) => !allowed.some((key) => f.endsWith(` takes ${key}`)))).toEqual([]);
      });
    }

  it("catches JSX text, text attributes and literal children, and leaves code alone", () => {
    const found = hardCoded(
      "probe.tsx",
      [
        "export const A = ({ on }: { on: boolean }) => (",
        '  <div title="Hello" aria-label={on ? "Shown" : "Hidden"} className="no-copy" data-testid="x-y">',
        "    Some text {on && \"Yes\"} {`Count ${1}`} {t('ok')} {'…'}",
        '    <input placeholder="Type here" />',
        '    <Card why={"Because"} id="raw-id" />',
        '    <p title={"Copy " + name}>{name ?? "Nobody"}{"Left" || name}</p>',
        '    <i title={`${label ?? "Unnamed wallet"}`} />',
        "  </div>",
        ");",
        'showToast("Copied"); other("not-copy");',
      ].join("\n"),
    );
    expect(found).toEqual(["2: Hello", "2: Shown", "2: Hidden", "3: Some text", "3: Yes", "3: Count", "4: Type here", "5: Because", "6: Copy", "6: Nobody", "6: Left", "7: Unnamed wallet", "10: Copied"]);
  });

  it("catches a copy button named from a noun and a message given another message", () => {
    const found = fragments(
      "probe.tsx",
      [
        "export const A = () => (",
        '  <div title={t("a.sentence", { theme: t("a.noun") })}>',
        '    <CopyValue value={v} label={t("a.address")} />',
        '    <CopyValue value={v} action={t("a.copyAddress")} />',
        '    {t.rich("a.rich", { who: tr("a.name"), count: 2, link: <b>{t("a.link")}</b> })}',
        '    {t("a.line", { note: t(`a.note.${kind}`) })}',
        '    {t("a.hint", { theme: t(dark ? "a.dark" : "a.light") })}',
        "  </div>",
        ");",
      ].join("\n"),
    );
    expect(found).toEqual(["2: a.sentence takes a.noun", "3: CopyValue label=", "5: a.rich takes a.name", "6: a.line takes a.note.…", "7: a.hint takes a.dark | a.light"]);
  });
});
