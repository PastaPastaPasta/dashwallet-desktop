// Gate U3(a): the UX-SPEC §2 checklist (colour roles, radii, elevation, type ramp, spacing) as
// computed-style assertions. They run inside the page, so the same list is checked in Chromium,
// WebKit (Playwright), WebKitGTK (`--selftest`), and on CI in WKWebView and WebView2.
// Expected values come from the generated tokens, never from literals.

import { buttons, elevation, layout, opacity, radius, roles, spacing, typography, type Role, type TextStyle } from "../styles/tokens.generated";

export type Screen = "overview" | "transactions" | "txDetail" | "unlock";
type Theme = "light" | "dark";

interface Check {
  id: string;
  /** UX-SPEC section the item comes from. */
  spec: string;
  screen: Screen;
  selector: string;
  property: string;
  expected: (theme: Theme) => string;
  kind: "color" | "px" | "exact" | "shadow" | "contains";
  /** The element exists only for some data (a status chip needs a pending transaction). */
  optional?: boolean;
}

export interface CheckResult {
  id: string;
  spec: string;
  selector: string;
  property: string;
  expected: string;
  actual: string;
  /** `null`: not applicable, the data produced no such element (only for optional checks). */
  pass: boolean | null;
}

const role = (r: Role) => (t: Theme) => roles[r][t];
const px = (n: number) => () => `${n}px`;
const same = (v: string) => () => v;
const white = (alpha: number) => () => `rgb(255 255 255 / ${alpha})`;

const color = (id: string, screen: Screen, selector: string, property: string, expected: (t: Theme) => string): Check => ({
  id,
  spec: "§2.1",
  screen,
  selector,
  property,
  expected,
  kind: "color",
});

const radiusCheck = (id: string, screen: Screen, selector: string, value: number): Check => ({
  id,
  spec: "§2.4",
  screen,
  selector,
  property: "border-top-left-radius",
  expected: px(value),
  kind: "px",
});

function type(id: string, screen: Screen, selector: string, style: TextStyle, weight?: number): Check[] {
  const t = typography[style];
  return [
    { id: `${id}.size`, spec: "§2.2", screen, selector, property: "font-size", expected: px(t.size), kind: "px" },
    { id: `${id}.weight`, spec: "§2.2", screen, selector, property: "font-weight", expected: same(String(weight ?? t.weight)), kind: "exact" },
    { id: `${id}.line`, spec: "§2.2", screen, selector, property: "line-height", expected: px(t.lineHeight), kind: "px" },
  ];
}

const space = (id: string, screen: Screen, selector: string, property: string, value: number): Check => ({
  id,
  spec: "§2.3",
  screen,
  selector,
  property,
  expected: px(value),
  kind: "px",
});

function shadow(id: string, screen: Screen, selector: string, level: keyof typeof elevation): Check {
  const e = elevation[level];
  return {
    id,
    spec: "§2.5",
    screen,
    selector,
    property: "box-shadow",
    expected: (t) => `${roles.shadow[t]} 0px ${e.y}px ${e.radius * 2}px 0px`,
    kind: "shadow",
  };
}

const optional = (checks: Check | Check[]): Check[] => (Array.isArray(checks) ? checks : [checks]).map((c) => ({ ...c, optional: true }));

export const CHECKS: Check[] = [
  // Colour roles (§2.1)
  color("canvas", "overview", '[data-testid="page-home"]', "background-color", role("canvas")),
  color("canvas.transactions", "transactions", '[data-testid="page-activity"]', "background-color", role("canvas")),
  color("card", "overview", ".tx-group", "background-color", role("card")),
  color("hero", "overview", '[data-testid="hero-band"]', "background-color", role("hero")),
  color("heroCard", "overview", '[data-testid="hero-breakdown"]', "background-color", white(opacity.heroCard)),
  color("textOnHero", "overview", '[data-testid="hero-amount"] .amount-number', "color", role("textOnHero")),
  color("textPrimary", "overview", ".tx-row-title", "color", role("textPrimary")),
  color("textSecondary", "overview", ".history-title", "color", role("textSecondary")),
  color("textSecondary.time", "overview", ".tx-row-time", "color", role("textSecondary")),
  color("textTertiary", "overview", ".tx-group-weekday", "color", role("textTertiary")),
  color("accent.sidebar", "overview", ".sidebar-item.is-current", "background-color", role("accent")),
  ...optional(color("accentTint.chip", "overview", ".chip-info", "background-color", role("accentTint"))),
  ...optional(color("textLink.chip", "overview", ".chip-info", "color", role("textLink"))),
  color("separator.statusBar", "overview", '[data-testid="status-bar"]', "border-top-color", role("separator")),
  color("shortcutCard", "overview", '[data-testid="shortcut-card"]', "background-color", role("card")),
  color("fieldFill.search", "transactions", '[data-testid="tx-search"]', "background-color", role("fieldFill")),
  color("accent.filterChip", "transactions", ".filter-chip.is-selected", "background-color", role("accent")),
  color("card.sheet", "txDetail", '[data-testid="tx-detail"]', "background-color", role("card")),
  color("overlay.sheet", "txDetail", ".sheet-scrim", "background-color", role("overlay")),
  color("hero.lock", "unlock", '[data-testid="lock-screen"]', "background-color", role("hero")),
  color("textOnHero.lock", "unlock", ".lock-title", "color", role("textOnHero")),

  // Radii (§2.4)
  radiusCheck("radius.group", "overview", ".tx-group", radius.group),
  radiusCheck("radius.card.shortcut", "overview", '[data-testid="shortcut-card"]', radius.card),
  radiusCheck("radius.standard.breakdown", "overview", '[data-testid="hero-breakdown"]', radius.standard),
  ...optional(radiusCheck("radius.switcher.chip", "overview", ".chip", radius.switcher)),
  radiusCheck("radius.searchField", "transactions", '[data-testid="tx-search"]', radius.searchField),
  radiusCheck("radius.card.sheet", "txDetail", '[data-testid="tx-detail"]', radius.card),
  radiusCheck("radius.card.facts", "txDetail", '[data-testid="tx-detail-facts"]', radius.card),
  radiusCheck("radius.textField.lock", "unlock", '[data-testid="lock-field"]', radius.textField),
  radiusCheck("radius.button.large", "unlock", '[data-testid="lock-submit"]', buttons.large.cornerRadius),

  // Elevation (§2.5): level 1 cards, level 2 floating shortcut card
  shadow("elevation.card", "overview", ".tx-group", "card"),
  shadow("elevation.floating", "overview", '[data-testid="shortcut-card"]', "floating"),

  // Type ramp (§2.2)
  ...type("type.hero", "overview", '[data-testid="hero-amount"] .amount-number', "largeTitle"),
  ...type("type.rowTitle", "overview", ".tx-row-title", "footnoteMedium"),
  ...type("type.rowTime", "overview", ".tx-row-time", "footnote"),
  ...type("type.rowAmount", "overview", ".tx-row-amount .amount-number", "footnoteMedium"),
  ...type("type.dayHeader", "overview", ".tx-group-day", "footnoteMedium"),
  ...type("type.weekday", "overview", ".tx-group-weekday", "footnote"),
  ...optional(type("type.chip", "overview", ".chip", "caption1Medium")),
  ...type("type.history", "overview", ".history-title", "subhead"),
  // Shortcut captions are caption2 semibold (UX-SPEC C6).
  ...type("type.shortcut", "overview", ".shortcut-title", "caption2", 600),
  ...type("type.breakdownTitle", "overview", ".breakdown-title", "footnoteMedium"),
  ...type("type.breakdownSubtitle", "overview", ".breakdown-subtitle", "caption2"),
  ...type("type.sidebar", "overview", ".sidebar-item", "subhead"),
  ...type("type.statusBar", "overview", '[data-testid="status-bar"]', "caption1"),
  ...type("type.pageTitle", "transactions", ".top-intro-title", "title2"),
  ...type("type.sheetTitle", "txDetail", ".sheet-title", "subheadMedium"),
  ...type("type.detailTitle", "txDetail", ".tx-detail-title", "title3"),
  ...type("type.detailAmount", "txDetail", ".tx-detail-amount .amount-number", "title1"),
  ...type("type.lockTitle", "unlock", ".lock-title", "title2"),
  ...type("type.lockPrompt", "unlock", ".lock-prompt", "subhead"),
  { id: "type.family", spec: "§2.2", screen: "overview", selector: "body", property: "font-family", expected: same("Inter"), kind: "contains" },
  // The capsule exists only off mainnet.
  ...optional({ id: "type.capsule.tracking", spec: "§2.2", screen: "overview", selector: '[data-testid="hero-capsule"]', property: "letter-spacing", expected: px(1.2), kind: "px" }),

  // Spacing and layout (§2.3)
  space("space.row.v", "overview", ".tx-row", "padding-top", spacing.transactionRowVertical),
  space("space.row.h", "overview", ".tx-row", "padding-left", spacing.transactionRowHorizontal),
  space("space.row.gap", "overview", ".tx-row", "column-gap", spacing.transactionRowSpacing),
  space("layout.txRowMinHeight", "overview", ".tx-row", "min-height", layout.txRowMinHeight),
  space("layout.dayHeaderHeight", "overview", ".tx-group-header", "height", layout.dayHeaderHeight),
  space("layout.contentMaxWidth", "overview", ".page-column", "max-width", layout.contentMaxWidth),
  space("layout.pagePaddingH", "overview", ".page-column", "padding-left", layout.pagePaddingH),
  space("layout.stripMaxWidth", "overview", '[data-testid="hero-breakdown"]', "max-width", layout.heroStripMaxWidth),
  space("layout.shortcutMaxWidth", "overview", '[data-testid="shortcut-card"]', "max-width", layout.shortcutCardMaxWidth),
  space("layout.statusBarHeight", "overview", '[data-testid="status-bar"]', "height", layout.statusBarHeight),
  space("layout.sidebarWidth", "overview", '[data-testid="sidebar"]', "width", layout.sidebarIdealWidth),
  space("layout.sectionGap.groups", "overview", ".tx-groups", "row-gap", spacing.stack),
  space("layout.cardPadding.facts", "txDetail", '[data-testid="tx-detail-facts"]', "padding-left", layout.cardPadding),
  space("layout.sheetWidthMedium", "txDetail", '[data-testid="tx-detail"]', "width", layout.sheetWidthMedium),
  space("layout.sheetHeader", "txDetail", '[data-testid="tx-detail-header"]', "height", layout.sheetHeaderHeight),
];

function rgba(s: string): [number, number, number, number] | null {
  if (s === "transparent") return [0, 0, 0, 0];
  // `color-mix()` results serialise as `color(srgb r g b / a)` with 0–1 channels.
  const srgb = /^color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)$/.exec(s.trim());
  if (srgb) return [Number(srgb[1]) * 255, Number(srgb[2]) * 255, Number(srgb[3]) * 255, srgb[4] === undefined ? 1 : Number(srgb[4])];
  const hex = /^#([0-9a-f]{6})$/i.exec(s.trim());
  if (hex) {
    const n = parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const nums = s.match(/-?[\d.]+%?/g);
  if (!nums || nums.length < 3) return null;
  // Percent channels are 0–255 at 100 %; a percent alpha is 0–1 at 100 %.
  const v = nums.map((x, i) => (x.endsWith("%") ? (Number(x.slice(0, -1)) / 100) * (i < 3 ? 255 : 1) : Number(x)));
  return [v[0], v[1], v[2], v[3] ?? 1];
}

const sameColor = (a: string, b: string) => {
  const x = rgba(a);
  const y = rgba(b);
  if (!x || !y) return false;
  // Fully transparent colours match whatever their channels.
  if (x[3] < 0.005 && y[3] < 0.005) return true;
  return x.slice(0, 3).every((c, i) => Math.abs(c - y[i]) <= 1) && Math.abs(x[3] - y[3]) <= 0.01;
};

function sameShadow(actual: string, expected: string): boolean {
  const exp = rgba(expected.split(" 0px")[0]);
  if (exp && exp[3] < 0.005) return actual === "none" || sameColor(actual.split(/\)\s/)[0] + ")", "transparent");
  const color = /^(rgba?\([^)]*\)|#[0-9a-f]+)/i.exec(actual)?.[1];
  const colorExp = /^(rgba?\([^)]*\)|#[0-9a-f]+)/i.exec(expected)?.[1];
  const lengths = (s: string) => (s.replace(/^(rgba?\([^)]*\)|#[0-9a-f]+)/i, "").match(/-?[\d.]+px/g) ?? []).map(parseFloat);
  return !!color && !!colorExp && sameColor(color, colorExp) && lengths(actual).join() === lengths(expected).join();
}

function compare(kind: Check["kind"], actual: string, expected: string): boolean {
  switch (kind) {
    case "color":
      return sameColor(actual, expected);
    case "px":
      return Math.abs(parseFloat(actual) - parseFloat(expected)) < 0.05;
    case "exact":
      return actual === expected;
    case "shadow":
      return sameShadow(actual, expected);
    case "contains":
      return actual.includes(expected);
  }
}

/** Runs the checks of `screen` against the live document in its current appearance. */
export function runStyleChecks(screen: Screen, theme: Theme): CheckResult[] {
  return CHECKS.filter((c) => c.screen === screen).map((c) => {
    const el = document.querySelector(c.selector);
    const expected = c.expected(theme);
    if (!el) return { id: c.id, spec: c.spec, selector: c.selector, property: c.property, expected, actual: "missing", pass: c.optional ? null : false };
    const actual = getComputedStyle(el).getPropertyValue(c.property).trim();
    return { id: c.id, spec: c.spec, selector: c.selector, property: c.property, expected, actual, pass: compare(c.kind, actual, expected) };
  });
}
