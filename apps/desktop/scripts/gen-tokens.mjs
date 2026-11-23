#!/usr/bin/env node
// gen-tokens.mjs — CSS custom properties and a typed token module for the Tauri renderer.
//
// Usage:
//   node scripts/gen-tokens.mjs           write the outputs
//   node scripts/gen-tokens.mjs --check   exit 1 when an output is stale (build and CI)
//
// Inputs (Resources/Tokens; never hand-copy a value into the renderer):
//   tokens.json              colours, type scale, icon list (scripts/gen-tokens.swift output)
//   roles.json               semantic colour roles (UX-SPEC §2.1, §2.1a), DashOpacity, DashOpacityAA
//   spacing.json             DashSpacing
//   radii.json               DashRadius
//   layout.json              DashLayout, DashMotion, DashElevation
//   component-metrics.json   DashButtonMetrics
//
// Outputs (relative to apps/desktop):
//   src/styles/tokens.generated.css   :root (light) and dark blocks of --dw-* properties
//   src/styles/tokens.generated.ts    the same values as data, for computed-style tests
//
// A role colour is a palette reference ("DashColor.<token>", "DashColor.App.<token>"), "#RRGGBB" or
// [r, g, b, a] (0…1); a role is one colour or { light, dark }. Anything else fails the script.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(appDir, "../..");
const tokensDir = join(repo, "Resources/Tokens");
const outCss = join(appDir, "src/styles/tokens.generated.css");
const outTs = join(appDir, "src/styles/tokens.generated.ts");

function fail(msg) {
  process.stderr.write(`gen-tokens: error: ${msg}\n`);
  process.exit(1);
}

// ---------------------------------------------------------------- colours (tokens.json)

const tokens = JSON.parse(readFileSync(join(tokensDir, "tokens.json"), "utf8"));

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

// ---------------------------------------------------------------- hand-written tokens (JSON)

const source = (file) => JSON.parse(readFileSync(join(tokensDir, file), "utf8"));
const block = (obj, file, name) => obj[name] ?? fail(`${file}: no ${name}`);

function numbers(obj, file, name) {
  const out = block(obj, file, name);
  for (const [k, v] of Object.entries(out)) if (typeof v !== "number") fail(`${file}: ${name}.${k} is not a number`);
  return out;
}

function sideColor(value, which) {
  if (typeof value === "string" && value.startsWith("DashColor.")) return paletteColor(value)[which];
  if (typeof value === "string") {
    const m = /^#([0-9A-Fa-f]{6})$/.exec(value) ?? fail(`not a colour: ${value}`);
    const n = parseInt(m[1], 16);
    return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255, 1];
  }
  if (Array.isArray(value) && value.length === 4 && value.every((v) => typeof v === "number")) return value;
  fail(`not a colour: ${JSON.stringify(value)}`);
}

function roleMembers(obj, name) {
  const out = {};
  for (const [role, value] of Object.entries(block(obj, "roles.json", name))) {
    const sides = typeof value === "object" && !Array.isArray(value) ? value : { light: value, dark: value };
    out[role] = { light: sideColor(sides.light, "light"), dark: sideColor(sides.dark, "dark") };
  }
  return out;
}

const roleSource = source("roles.json");
// The WCAG AA variants (UX-SPEC §2.1a, DEC-36) replace the iOS values: the renderer draws only these.
const opacity = { ...numbers(roleSource, "roles.json", "DashOpacity"), ...numbers(roleSource, "roles.json", "DashOpacityAA") };
const roles = { ...roleMembers(roleSource, "Role"), ...roleMembers(roleSource, "RoleAA") };

const spacing = numbers(source("spacing.json"), "spacing.json", "DashSpacing");
const radius = numbers(source("radii.json"), "radii.json", "DashRadius");
const layoutSource = source("layout.json");
const layout = numbers(layoutSource, "layout.json", "DashLayout");
const motion = numbers(layoutSource, "layout.json", "DashMotion");

const elevation = {};
for (const [name, e] of Object.entries(block(layoutSource, "layout.json", "DashElevation"))) {
  if (typeof e.radius !== "number" || typeof e.y !== "number" || typeof e.borderInDark !== "boolean") {
    fail(`layout.json: DashElevation.${name} needs radius, y and borderInDark`);
  }
  elevation[name] = { radius: e.radius, y: e.y, borderInDark: e.borderInDark };
}

const buttonFields = ["horizontalPadding", "verticalPadding", "gap", "cornerRadius", "fontSize"];
const buttons = {};
for (const [name, b] of Object.entries(block(source("component-metrics.json"), "component-metrics.json", "DashButtonMetrics"))) {
  buttons[name] = Object.fromEntries(
    buttonFields.map((f) => [f, typeof b[f] === "number" ? b[f] : fail(`component-metrics.json: DashButtonMetrics.${name}.${f} is not a number`)]),
  );
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
