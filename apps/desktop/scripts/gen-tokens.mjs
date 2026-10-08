#!/usr/bin/env node
// gen-tokens.mjs — CSS custom properties and a typed token module for the Tauri renderer.
//
// Usage:
//   node scripts/gen-tokens.mjs           write the outputs
//   node scripts/gen-tokens.mjs --check   exit 1 when an output is stale (build and CI)
//
// Inputs (the same sources the Swift toolkits read; never hand-copy a value):
//   Resources/Tokens/tokens.json            colours, type scale, icon list (scripts/gen-tokens.swift output)
//   Sources/DesignTokens/Roles.swift        semantic colour roles (UX-SPEC §2.1) and DashOpacity
//   Sources/DesignTokens/Spacing.swift      DashSpacing
//   Sources/DesignTokens/Radii.swift        DashRadius
//   Sources/DesignTokens/Layout.swift       DashLayout, DashMotion, DashElevation, DashRadius.group
//   Sources/DesignTokens/ComponentMetrics.swift  DashButtonMetrics
//
// Outputs (relative to apps/desktop):
//   src/styles/tokens.generated.css   :root (light) and dark blocks of --dw-* properties
//   src/styles/tokens.generated.ts    the same values as data, for computed-style tests
//
// The Swift files are parsed, not executed: only `public static let name[: Double] = <number>`,
// role aliases (`DashColor.<token>`, `DashColor.App.<token>`, `DashColor(light: …, dark: …)`,
// `RGBA(red:green:blue:alpha:)`) and `Type(label: value, …)` constructors are understood. Anything
// else in a parsed block fails the script instead of being skipped.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(appDir, "../..");
const tokensDir = join(repo, "Sources/DesignTokens");
const outCss = join(appDir, "src/styles/tokens.generated.css");
const outTs = join(appDir, "src/styles/tokens.generated.ts");

function fail(msg) {
  process.stderr.write(`gen-tokens: error: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------- colours (tokens.json)

const tokens = JSON.parse(readFileSync(join(repo, "Resources/Tokens/tokens.json"), "utf8"));

/** `{ light, dark }` RGBA tuples (0…1) per catalog token. */
const palette = { DashColor: new Map(), "DashColor.App": new Map() };
for (const [catalog, list] of Object.entries(tokens.colors)) {
  if (!palette[catalog]) fail(`unknown colour catalog ${catalog}`);
  for (const c of list) palette[catalog].set(c.token, { light: c.light.srgb, dark: c.dark.srgb });
}

function paletteColor(path) {
  // "DashColor.App.secondaryBackgroundColor" or "DashColor.blue"
  const m = /^DashColor\.(?:(App)\.)?(\w+)$/.exec(path);
  if (!m) fail(`not a palette reference: ${path}`);
  const catalog = m[1] ? "DashColor.App" : "DashColor";
  const c = palette[catalog].get(m[2]);
  if (!c) fail(`unknown colour ${path}`);
  return c;
}

const round = (x, digits = 4) => Number(x.toFixed(digits));

function css(rgba) {
  const [r, g, b, a] = rgba.map((v) => v);
  const to8 = (v) => Math.round(v * 255);
  if (a >= 1) return `#${[r, g, b].map((v) => to8(v).toString(16).padStart(2, "0")).join("").toUpperCase()}`;
  return `rgb(${to8(r)} ${to8(g)} ${to8(b)} / ${round(a, 3)})`;
}

// ---------------------------------------------------------------- Swift parsing

/** Statements (joined across lines until parentheses balance) inside each named block. */
function swiftBlocks(file) {
  const text = readFileSync(join(tokensDir, file), "utf8");
  const blocks = new Map();
  // One entry per open brace: a type name for enum/struct/extension bodies, null for any
  // other body (init, func, computed property), whose statements are not tokens.
  const stack = [];
  let pending = "";
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\/\/.*$/, "").trim();
    if (!line) continue;
    if (!pending) {
      const open = /^(?:public\s+)?(?:enum|struct|extension)\s+([\w.]+)[^{]*\{$/.exec(line);
      if (open) {
        stack.push(open[1]);
        continue;
      }
      if (line === "}") {
        stack.pop();
        continue;
      }
      if (line.endsWith("{")) {
        stack.push(null);
        continue;
      }
    }
    pending = pending ? `${pending} ${line}` : line;
    const depth = (pending.match(/[([]/g) ?? []).length - (pending.match(/[)\]]/g) ?? []).length;
    if (depth > 0) continue;
    if (!stack.includes(null)) {
      const name = stack.join(".");
      if (!blocks.has(name)) blocks.set(name, []);
      blocks.get(name).push(pending);
    }
    pending = "";
  }
  return blocks;
}

const STATIC_NUMBER = /^public static let (\w+)(?:: Double)? = (-?[\d.]+)$/;

/** Numeric `public static let` members of `block`; other `public static let`s fail unless skipped. */
function numbers(blocks, block, { skip = [] } = {}) {
  const out = {};
  for (const stmt of blocks.get(block) ?? fail(`no block ${block}`)) {
    if (!stmt.startsWith("public static let")) continue;
    const m = STATIC_NUMBER.exec(stmt);
    if (m) out[m[1]] = Number(m[2]);
    else if (!skip.some((s) => stmt.startsWith(`public static let ${s}`))) fail(`${block}: cannot read ${stmt}`);
  }
  return out;
}

/** `Type(label: value, …)` → `{ label: value }`; values are numbers, booleans or raw text. */
function constructorArgs(text, type) {
  const m = new RegExp(`^${type}\\((.*)\\)$`).exec(text.trim());
  if (!m) fail(`not a ${type}(…) constructor: ${text}`);
  const args = {};
  let depth = 0;
  let current = "";
  for (const ch of m[1] + ",") {
    if (ch === "(") depth++;
    if (ch === ")") depth--;
    if (ch === "," && depth === 0) {
      const [label, ...rest] = current.split(":");
      const value = rest.join(":").trim();
      args[label.trim()] = /^-?[\d.]+$/.test(value) ? Number(value) : value === "true" ? true : value === "false" ? false : value;
      current = "";
    } else current += ch;
  }
  return args;
}

function rgbaOf(text) {
  const a = constructorArgs(text, "RGBA");
  return [a.red, a.green, a.blue, a.alpha ?? 1];
}

/** A role's value: `DashColor.<t>`, `DashColor(light: X.light, dark: Y.dark)` or with RGBA(…) sides. */
function roleColor(expr) {
  if (/^DashColor\.[\w.]+$/.test(expr)) return paletteColor(expr);
  const args = constructorArgs(expr, "DashColor");
  const side = (value, which) => {
    if (value.startsWith("RGBA(")) return rgbaOf(value);
    const m = new RegExp(`^(DashColor\\.[\\w.]+)\\.${which}$`).exec(value);
    if (!m) fail(`role side not understood: ${value}`);
    return paletteColor(m[1])[which];
  };
  return { light: side(args.light, "light"), dark: side(args.dark, "dark") };
}

const roleBlocks = swiftBlocks("Roles.swift");
const opacity = numbers(roleBlocks, "DashOpacity");
const roles = {};
for (const stmt of roleBlocks.get("DashColor.Role") ?? fail("no DashColor.Role block")) {
  const m = /^public static let (\w+) = (.+)$/.exec(stmt);
  if (!m) continue;
  if (m[1] === "all") continue; // the list of roles for tests
  roles[m[1]] = roleColor(m[2]);
}

const spacing = numbers(swiftBlocks("Spacing.swift"), "DashSpacing");
const radius = numbers(swiftBlocks("Radii.swift"), "DashRadius");
const layoutBlocks = swiftBlocks("Layout.swift");
const layout = numbers(layoutBlocks, "DashLayout");
const motion = numbers(layoutBlocks, "DashMotion");
Object.assign(radius, numbers(layoutBlocks, "DashRadius"));

const elevation = {};
for (const stmt of layoutBlocks.get("DashElevation") ?? []) {
  const m = /^public static let (\w+) = (DashElevation\(.+\))$/.exec(stmt);
  if (!m) continue;
  const a = constructorArgs(m[2], "DashElevation");
  if (a.color !== "DashColor.Role.shadow") fail(`elevation ${m[1]}: unexpected colour ${a.color}`);
  elevation[m[1]] = { radius: a.radius, y: a.y, borderInDark: a.borderInDark };
}

const buttons = {};
for (const stmt of swiftBlocks("ComponentMetrics.swift").get("DashButtonMetrics") ?? []) {
  const m = /^public static let (\w+) = (DashButtonMetrics\(.+\))$/.exec(stmt);
  if (m) buttons[m[1]] = constructorArgs(m[2], "DashButtonMetrics");
}

const weights = { regular: 400, medium: 500, semibold: 600, bold: 700 };
const typography = {};
for (const t of tokens.typography) {
  if (!(t.weight in weights)) fail(`unknown weight ${t.weight}`);
  typography[t.name] = { size: t.size, weight: weights[t.weight], lineHeight: t.lineHeight, tracking: t.tracking };
}

// ---------------------------------------------------------------- output

const kebab = (s) => s.replace(/([a-z0-9])([A-Z])/g, "$1-$2").replace(/([A-Z])([A-Z][a-z])/g, "$1-$2").toLowerCase();
const px = (n) => `${n}px`;

function colorBlock(which) {
  const lines = [];
  for (const [catalog, map] of Object.entries(palette)) {
    const prefix = catalog === "DashColor" ? "--dw-c-" : "--dw-c-app-";
    for (const [token, c] of map) lines.push(`  ${prefix}${kebab(token)}: ${css(c[which])};`);
  }
  for (const [name, c] of Object.entries(roles)) lines.push(`  --dw-role-${kebab(name)}: ${css(c[which])};`);
  // CSS blur ≈ 2 × the CoreAnimation/SwiftUI shadow radius.
  for (const [name, e] of Object.entries(elevation)) {
    lines.push(`  --dw-shadow-${kebab(name)}: 0 ${px(e.y)} ${px(e.radius * 2)} ${css(roles.shadow[which])};`);
    lines.push(`  --dw-border-${kebab(name)}: ${which === "dark" && e.borderInDark ? `1px solid ${css(roles.cardBorder.dark)}` : "0 solid transparent"};`);
  }
  return lines.join("\n");
}

function scalarBlock() {
  const lines = [`  --dw-font-family: "Inter", system-ui, sans-serif;`];
  lines.push(`  --dw-font-mono: ui-monospace, "SF Mono", "Cascadia Mono", "DejaVu Sans Mono", monospace;`);
  for (const [n, v] of Object.entries(spacing)) lines.push(`  --dw-space-${kebab(n)}: ${px(v)};`);
  for (const [n, v] of Object.entries(radius)) lines.push(`  --dw-radius-${kebab(n)}: ${px(v)};`);
  for (const [n, v] of Object.entries(layout)) lines.push(`  --dw-layout-${kebab(n)}: ${px(v)};`);
  for (const [n, v] of Object.entries(opacity)) lines.push(`  --dw-opacity-${kebab(n)}: ${v};`);
  for (const [n, v] of Object.entries(motion)) {
    const isTime = !/Scale|Opacity|Damping|PulseLow|PulseHigh/.test(n);
    lines.push(`  --dw-motion-${kebab(n)}: ${isTime ? `${v}s` : v};`);
  }
  for (const [n, t] of Object.entries(typography)) {
    const k = kebab(n);
    lines.push(`  --dw-type-${k}-size: ${px(t.size)};`, `  --dw-type-${k}-weight: ${t.weight};`, `  --dw-type-${k}-line: ${px(t.lineHeight)};`);
  }
  for (const [n, b] of Object.entries(buttons)) {
    const k = kebab(n);
    lines.push(
      `  --dw-button-${k}-padding: ${px(b.verticalPadding)} ${px(b.horizontalPadding)};`,
      `  --dw-button-${k}-gap: ${px(b.gap)};`,
      `  --dw-button-${k}-radius: ${px(b.cornerRadius)};`,
      `  --dw-button-${k}-font: ${px(b.fontSize)};`,
    );
  }
  return lines.join("\n");
}

const header = "/* Generated by apps/desktop/scripts/gen-tokens.mjs from Resources/Tokens/tokens.json and Sources/DesignTokens. Do not edit. */";
const darkBody = colorBlock("dark");
const cssOut = `${header}
:root {
  color-scheme: light;
${scalarBlock()}
${colorBlock("light")}
}

:root[data-theme="dark"] {
  color-scheme: dark;
${darkBody}
}

@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
${darkBody.replace(/^ {2}/gm, "    ")}
  }
}
`;

const resolvedRoles = Object.fromEntries(Object.entries(roles).map(([n, c]) => [n, { light: css(c.light), dark: css(c.dark) }]));
const tsOut = `// Generated by apps/desktop/scripts/gen-tokens.mjs. Do not edit.
// Values for tests and code that needs a number; styling goes through tokens.generated.css.

export const roles = ${JSON.stringify(resolvedRoles, null, 2)} as const;
export type Role = keyof typeof roles;

export const spacing = ${JSON.stringify(spacing, null, 2)} as const;
export const radius = ${JSON.stringify(radius, null, 2)} as const;
export const layout = ${JSON.stringify(layout, null, 2)} as const;
export const opacity = ${JSON.stringify(opacity, null, 2)} as const;
export const motion = ${JSON.stringify(motion, null, 2)} as const;
export const elevation = ${JSON.stringify(elevation, null, 2)} as const;
export const typography = ${JSON.stringify(typography, null, 2)} as const;
export type TextStyle = keyof typeof typography;
export const buttons = ${JSON.stringify(buttons, null, 2)} as const;

/** Exported icon files (Resources/Icons) by token id. */
export const icons = ${JSON.stringify(
  Object.fromEntries(tokens.icons.map((i) => [i.id, { file: i.file, dark: i.hasDarkVariant, template: i.isTemplate }])),
  null,
  2,
)} as const;
export type IconId = keyof typeof icons;
`;

const check = process.argv.includes("--check");
let stale = false;
for (const [path, text] of [
  [outCss, cssOut],
  [outTs, tsOut],
]) {
  let current = "";
  try {
    current = readFileSync(path, "utf8");
  } catch {
    /* missing */
  }
  if (current === text) continue;
  if (check) {
    process.stderr.write(`gen-tokens: ${path} is stale; run \`pnpm tokens\`\n`);
    stale = true;
  } else {
    writeFileSync(path, text);
    process.stdout.write(`gen-tokens: wrote ${path}\n`);
  }
}
process.exit(stale ? 1 : 0);
