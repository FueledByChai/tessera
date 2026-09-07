#!/usr/bin/env node
// Terminal-theme discipline check (UI-01). Parses app/globals.css and fails when:
//   1. a background or border colour that applies in terminal mode has a blue hue, unless the
//      rule is a selection or hover highlight (.active, .selected, :hover, heat-map cells);
//   2. a terminal-mode form control (input, select, textarea) rule sets a font-size below
//      18px in the form grids (15px for compact toolbar controls), or a label below 15px;
//   3. outside the terminal scope (so in modern mode, and as the base the terminal inherits)
//      the last rule in source order that sizes a form control, compact control, or label
//      falls below those same minimums, or no such rule exists.
// Colours are checked in the rules scoped to [data-theme="terminal"] (with the terminal's own
// CSS variables resolved) and in unscoped rules, which apply in terminal mode too. The modern
// theme's token block on bare `.app-shell` and the body/html ground are its own palette.
//
//   node web/scripts/theme-check.mjs            report and exit non-zero on failure
//   node web/scripts/theme-check.mjs --list     also print every colour it examined
import { readFileSync } from "node:fs";

const cssPath = new URL("../app/globals.css", import.meta.url);
const source = readFileSync(cssPath, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const listAll = process.argv.includes("--list");

/** Flat list of {selector, declarations, line} rules, at-rules descended into. */
function parseRules(text, offset = 0, out = []) {
  let i = 0;
  while (i < text.length) {
    const open = text.indexOf("{", i);
    if (open < 0) break;
    let depth = 1;
    let j = open + 1;
    while (j < text.length && depth > 0) {
      if (text[j] === "{") depth += 1;
      else if (text[j] === "}") depth -= 1;
      j += 1;
    }
    const selector = text.slice(i, open).trim().replace(/^[;\s]+/, "");
    const body = text.slice(open + 1, j - 1);
    const line = offset + text.slice(0, open).split("\n").length;
    if (selector.startsWith("@")) {
      if (/^@(media|supports|layer)/.test(selector)) parseRules(body, line - 1, out);
    } else {
      const declarations = body
        .split(";")
        .map((d) => d.trim())
        .filter(Boolean)
        .map((d) => {
          const colon = d.indexOf(":");
          return { property: d.slice(0, colon).trim(), value: d.slice(colon + 1).trim() };
        })
        .filter((d) => d.property);
      out.push({ selector, declarations, line });
    }
    i = j;
  }
  return out;
}

const rules = parseRules(source);
const isTerminal = (selector) => selector.includes('[data-theme="terminal"]');
const isOtherTheme = (selector) => /data-theme=/.test(selector) && !isTerminal(selector);
const isModernTokenBlock = (selector) =>
  selector
    .split(",")
    .map((s) => s.trim())
    .every((s) => ["html", "body", ".app-shell", ":root", "html, body"].includes(s));

// Terminal custom properties: last declaration wins, as in the cascade.
const tokens = new Map();
for (const rule of rules) {
  if (!isTerminal(rule.selector)) continue;
  for (const d of rule.declarations) {
    if (d.property.startsWith("--")) tokens.set(d.property, d.value);
  }
}
function resolveVars(value, depth = 0) {
  if (depth > 5) return value;
  return value.replace(/var\((--[\w-]+)(?:,\s*([^)]+))?\)/g, (_, name, fallback) => {
    const found = tokens.get(name);
    return found != null ? resolveVars(found, depth + 1) : fallback ?? "";
  });
}

function parseColor(text) {
  const hex = text.match(/^#([0-9a-f]{3,8})$/i);
  if (hex) {
    let h = hex[1];
    if (h.length === 3 || h.length === 4) h = [...h].map((c) => c + c).join("");
    return [0, 2, 4].map((k) => parseInt(h.slice(k, k + 2), 16) / 255);
  }
  const rgb = text.match(/^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)/i);
  if (rgb) return [rgb[1], rgb[2], rgb[3]].map((v) => Number(v) / 255);
  return null;
}
function hsl([r, g, b]) {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return { h: 0, s: 0, l };
  const s = d / (1 - Math.abs(2 * l - 1));
  let h;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h = (h * 60 + 360) % 360;
  return { h, s, l };
}
/** Navy through azure: hue 190-280 with visible saturation and any lightness above black. */
function isBlue(color) {
  const { h, s, l } = hsl(color);
  return h >= 190 && h <= 280 && s >= 0.25 && l >= 0.03;
}

const COLOR_PROPERTIES = /^(background|background-color|background-image|border|border-color|border-(top|right|bottom|left)(-color)?|outline|outline-color)$/;
const HIGHLIGHT = /:hover|\.active|\.selected|\.heat-|\.starred|:focus/;
const colorTokenPattern = /#[0-9a-f]{3,8}\b|rgba?\([^)]*\)/gi;

// An unscoped rule is the modern theme's when a terminal-scoped rule re-declares the same
// property family for the same selector tail (`.panel` under `.app-shell[data-theme="terminal"]`).
const family = (property) => (property.startsWith("border") ? "border" : property.startsWith("outline") ? "outline" : "background");
const norm = (selector) => selector.replace(/\s+/g, " ").trim();
const terminalTails = [];
for (const rule of rules) {
  if (!isTerminal(rule.selector)) continue;
  const families = new Set(rule.declarations.filter((d) => COLOR_PROPERTIES.test(d.property)).map((d) => family(d.property)));
  if (!families.size) continue;
  for (const part of rule.selector.split(",")) {
    terminalTails.push({ tail: norm(part).replace(/^\.app-shell\[data-theme="terminal"\]\s*/, ""), families });
  }
}
const overriddenInTerminal = (selector, property) =>
  selector.split(",").every((part) => {
    const u = norm(part);
    return terminalTails.some((t) => (t.tail === u || t.tail.endsWith(" " + u)) && t.families.has(family(property)));
  });

const failures = [];
const examined = [];
for (const rule of rules) {
  if (isOtherTheme(rule.selector) || isModernTokenBlock(rule.selector)) continue;
  const terminal = isTerminal(rule.selector);
  for (const d of rule.declarations) {
    if (!COLOR_PROPERTIES.test(d.property)) continue;
    if (!terminal && overriddenInTerminal(rule.selector, d.property)) continue;
    // Unscoped rules apply in every theme, but their var() references resolve per theme, so only
    // their literal colours are the terminal theme's business.
    const value = terminal ? resolveVars(d.value) : d.value;
    for (const token of value.match(colorTokenPattern) ?? []) {
      const color = parseColor(token);
      if (!color) continue;
      examined.push(`${rule.line}: ${rule.selector} { ${d.property}: ${token} }`);
      if (isBlue(color) && !HIGHLIGHT.test(rule.selector)) {
        failures.push(
          `blue ${d.property} ${token} at line ${rule.line}: ${rule.selector.replace(/\s+/g, " ")}`,
        );
      }
    }
  }
}

// Terminal token values themselves (a navy --panel is the root of the problem).
for (const name of ["--bg", "--panel", "--panel-2", "--line", "--line-soft"]) {
  const value = tokens.get(name);
  const color = value ? parseColor(value) : null;
  if (color && isBlue(color)) failures.push(`blue terminal token ${name}: ${value}`);
}

// Form controls: terminal-scoped rules that size inputs, selects, textareas, and labels.
const px = (value) => {
  const m = value.match(/([\d.]+)px/);
  return m ? Number(m[1]) : null;
};
const controlSelector = /(^|[\s>+~])(input|select|textarea)(?![\w-])/;
const compactControl = /\.terminal-panel-title|\.catalog-select|\.catalog-segment|\.instrument-search|\.catalog-search|\.check-row|\.toggle-label|\.preset-save/;
const editor = /\.rust-editor/;
for (const rule of rules) {
  if (!isTerminal(rule.selector)) continue;
  for (const d of rule.declarations) {
    let size = null;
    if (d.property === "font-size") size = px(d.value);
    else if (d.property === "font") size = px(d.value);
    if (size == null) continue;
    const selectors = rule.selector.split(",").map((s) => s.trim());
    for (const sel of selectors) {
      if (controlSelector.test(sel) && !editor.test(sel)) {
        const minimum = compactControl.test(sel) ? 15 : 18;
        if (size < minimum) {
          failures.push(`form control font ${size}px < ${minimum}px at line ${rule.line}: ${sel}`);
        }
      } else if (/label(?![\w-])\s*$/.test(sel) && !/\.check-row|\.toggle-label|\.comparison-picker|\.source-picker/.test(sel)) {
        // The label itself; hints inside it (label > small, label > span) are not captions.
        if (size < 15) failures.push(`form label font ${size}px < 15px at line ${rule.line}: ${sel}`);
      }
    }
  }
}

// Both modes (UI-02): outside the terminal scope, the last rule in source order that sizes a
// form control, a compact control, or a label is taken as the effective one and must meet the
// same minimums. The theme-neutral sizing block therefore stays at the end of the stylesheet.
const lastNeutral = { control: null, compact: null, label: null };
for (const rule of rules) {
  if (isTerminal(rule.selector) || isOtherTheme(rule.selector)) continue;
  for (const d of rule.declarations) {
    const size = d.property === "font-size" || d.property === "font" ? px(d.value) : null;
    if (size == null) continue;
    for (const sel of rule.selector.split(",").map((s) => s.trim())) {
      if (controlSelector.test(sel) && !editor.test(sel)) {
        lastNeutral[compactControl.test(sel) ? "compact" : "control"] = { size, line: rule.line, sel };
      } else if (/label(?![\w-])\s*$/.test(sel) && !/\.check-row|\.toggle-label|\.comparison-picker|\.source-picker/.test(sel)) {
        lastNeutral.label = { size, line: rule.line, sel };
      }
    }
  }
}
for (const [kind, minimum] of [["control", 18], ["compact", 15], ["label", 15]]) {
  const last = lastNeutral[kind];
  if (!last) failures.push(`modern mode: no theme-neutral rule sizes form ${kind}s`);
  else if (last.size < minimum) {
    failures.push(`modern mode: last ${kind} size ${last.size}px < ${minimum}px at line ${last.line}: ${last.sel}`);
  }
}

if (listAll) {
  for (const line of examined) console.log(line);
}
if (failures.length) {
  console.error(`theme-check: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(
  `theme-check: ok (${examined.length} colours examined; effective sizes control ${lastNeutral.control?.size}px, compact ${lastNeutral.compact?.size}px, label ${lastNeutral.label?.size}px)`,
);
