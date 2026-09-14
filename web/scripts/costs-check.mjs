#!/usr/bin/env node
// Cost profile dialog check (UI-12, decisions 0003 and 0024). Serves the built bundle with
// /api/cost-profiles answered from web/fixtures/cost-profiles.json (seven profiles spanning
// all three models: four fixed tick + per unit, of which one is a user profile, two all-in
// basis points, and one costs off) and its POST answered from the same fixture, opens the Costs
// page in terminal and modern mode, and proves the create form is a dialog whose fields follow
// the chosen model:
//   1. the page shows no create form outside the dialog, only the profile cards;
//   2. New profile opens a native modal dialog;
//   3. All-in basis points shows no tick size, slippage, commission, or minimum field, Fixed
//      tick + per unit shows exactly those six, and Costs off shows only name, asset class,
//      and model;
//   4. switching model away and back restores the values typed;
//   5. a name the library already holds disables Save with the reason beside it;
//   6. a POST answered 400 renders the service's message above the buttons with the dialog
//      still open and every field holding what was typed;
//   7. Save posts the model's fields, closes the dialog, and the new profile joins the grid.
// Every other API path proxies to the console at LAYOUT_CONSOLE when one answers and returns
// 503 otherwise. Needs a Playwright-compatible Chromium like the layout check; without one, or
// without a built bundle, it reports that it skipped and exits 0.
//
//   node web/scripts/costs-check.mjs
//   node web/scripts/costs-check.mjs --verbose      print the fields and the posted body
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LAUNCH, reachable, readBody, resolveChromium, serveDist } from "./headless.mjs";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const fixturePath = fileURLToPath(new URL("../fixtures/cost-profiles.json", import.meta.url));
const verbose = process.argv.includes("--verbose");

/** The three models, and the fields each one shows beyond name, asset class, and model. */
const MODELS = [
  { value: "all_in_bps", name: "All-in basis points", fields: ["entry_bps", "exit_bps"] },
  {
    value: "fixed_tick_per_unit",
    name: "Fixed tick + per unit",
    fields: [
      "tick_size",
      "entry_slippage_ticks",
      "exit_slippage_ticks",
      "entry_commission_per_unit",
      "exit_commission_per_unit",
      "minimum_commission",
    ],
  },
  { value: "none", name: "Costs off", fields: [] },
];
const ALWAYS = ["name", "asset_class", "model"];
/** The fields no model but fixed tick + per unit may show. */
const ONLY_FIXED = ["tick_size", "entry_slippage_ticks", "exit_slippage_ticks", "entry_commission_per_unit", "exit_commission_per_unit", "minimum_commission"];
/** A name the fixture already holds, so Save has to be refused before it is sent. */
const DUPLICATE = "US equities conservative";
/** A name the fixture service answers 400 for, so the refusal is rendered in the dialog. */
const REFUSED = "Service refuses this one";
const REFUSAL = "a cost profile with that name already exists";
/** A name nothing holds, so the save goes through. */
const FRESH = "Desk override · new version";
/** What the check types, so a model switch can be proven to keep it. */
const TYPED = {
  all_in_bps: { entry_bps: "7.5", exit_bps: "2.5" },
  fixed_tick_per_unit: { tick_size: "0.25", entry_slippage_ticks: "3" },
};

const runtime = await resolveChromium();
if (!runtime) {
  console.log("costs-check: skipped (no playwright or patchright Chromium found)");
  process.exit(0);
}
if (!existsSync(join(distDir, "index.html"))) {
  console.log("costs-check: skipped (no web/dist; run npm run build first)");
  process.exit(0);
}

const profiles = JSON.parse(readFileSync(fixturePath, "utf8"));
/** Profiles the page posted, in order. */
const posted = [];
const json = (body, status = 200) => ({ status, type: "application/json", body: JSON.stringify(body) });
const fixtureApi = async (pathname, req) => {
  if (pathname === "/api/cost-profiles" && req.method === "POST") {
    const body = JSON.parse(await readBody(req));
    posted.push(body);
    if (String(body?.name ?? "").trim() === REFUSED) return json({ error: REFUSAL }, 400);
    const seed = profiles.find((profile) => profile.model === body.model) ?? profiles[0];
    return json(
      {
        ...seed,
        ...body,
        id: `cost-fixture-${posted.length}`,
        created_at: new Date().toISOString(),
        builtin: false,
      },
      201,
    );
  }
  if (pathname === "/api/cost-profiles") return json(profiles);
  if (pathname === "/api/jobs") return json([]);
  if (pathname === "/api/runs") return json([]);
  if (pathname === "/api/automations") return json([]);
  if (pathname === "/api/dashboard") {
    return json({ strategies: [], recent_runs: [], jobs: [], production_strategies: 0, archived_strategies: 0, historical_reports: 0, active_jobs: 0, worker_capacity: 2 });
  }
  return null;
};

const dialog = "dialog.cost-dialog[open]";
/** The fields the open dialog shows, in document order. */
const readFields = (page) =>
  page.evaluate((sel) => [...document.querySelectorAll(`${sel} .field-grid [data-field]`)].map((el) => el.dataset.field), dialog);
/** What one field holds: text for a select, the value for an input. */
const readValue = (page, field) => page.locator(`${dialog} [data-field="${field}"] input, ${dialog} [data-field="${field}"] select`).first().inputValue();
/** Types into one field. */
const fill = async (page, field, value) => {
  await page.locator(`${dialog} [data-field="${field}"] input`).first().fill(value);
};
/** Chooses the model and lets the field set settle. */
async function chooseModel(page, value) {
  await page.locator(`${dialog} [data-field="model"] select`).first().selectOption(value);
  await page.waitForTimeout(120);
}

const upstream = (await reachable(consoleOrigin)) ? consoleOrigin : null;
const served = await serveDist(distDir, upstream, fixtureApi);
const browser = await runtime.chromium.launch(LAUNCH);
const failures = [];
try {
  for (const mode of ["terminal", "modern"]) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await context.addInitScript((m) => window.localStorage.setItem("bt-display-mode", m), mode);
    const page = await context.newPage();
    const errors = [];
    page.on("console", (message) => {
      if (message.type() === "error" && !/Failed to load resource/.test(message.text())) errors.push(message.text());
    });
    page.on("pageerror", (error) => errors.push(String(error)));
    const fail = (text) => failures.push(`${mode}: ${text}`);
    try {
      await page.goto(served.url, { waitUntil: "domcontentloaded" });
      await page.locator(".app-shell").waitFor({ timeout: 15000 });
      await page.getByRole("button", { name: /^Costs$/ }).first().click();
      await page.locator(".costs-workspace").waitFor({ timeout: 15000 });
      await page.waitForTimeout(200);

      // 1. No create form on the page: every form on it is inside the dialog.
      const onPage = await page.evaluate(() => ({
        forms: [...document.querySelectorAll(".costs-workspace form")].filter((f) => !f.closest("dialog")).length,
        inputs: [...document.querySelectorAll(".costs-workspace input, .costs-workspace select")].filter((el) => !el.closest("dialog")).length,
        cards: document.querySelectorAll(".costs-workspace .cost-profile-card").length,
      }));
      if (onPage.forms) fail(`the Costs page still renders ${onPage.forms} create form(s) outside the dialog`);
      if (onPage.inputs) fail(`the Costs page still renders ${onPage.inputs} form control(s) outside the dialog`);
      if (onPage.cards !== profiles.length) fail(`the Costs page shows ${onPage.cards} profile card(s), the fixture holds ${profiles.length}`);

      // 2. New profile opens a native modal dialog.
      if (await page.locator(dialog).count()) fail("the dialog is open before New profile is clicked");
      await page.locator(".cost-dialog-open").first().click();
      await page.locator(dialog).waitFor({ timeout: 10000 });
      await page.waitForTimeout(150);
      const shape = await page.evaluate((sel) => {
        const el = document.querySelector(sel);
        return {
          modal: el?.matches(":modal") ?? false,
          cancel: Boolean(el?.querySelector(".cost-dialog-cancel")),
          save: Boolean(el?.querySelector(".cost-dialog-save")),
          close: Boolean(el?.querySelector(".cost-dialog-close")),
        };
      }, dialog);
      if (!shape.modal) fail("the dialog is not shown as a modal (showModal)");
      if (!shape.cancel || !shape.save) fail("the dialog lacks its Cancel or Save button");
      if (!shape.close) fail("the dialog lacks a close button in its title bar");

      // 3. Each model's field set.
      for (const model of MODELS) {
        await chooseModel(page, model.value);
        const fields = await readFields(page);
        if (verbose) console.log(`${mode}: ${model.value} shows ${JSON.stringify(fields)}`);
        const extra = fields.filter((field) => !ALWAYS.includes(field));
        if (extra.join("|") !== model.fields.join("|")) {
          fail(`${model.name} shows ${JSON.stringify(extra)}, not ${JSON.stringify(model.fields)}`);
        }
        if (fields.join("|") !== [...ALWAYS, ...model.fields].join("|")) {
          fail(`${model.name} shows ${JSON.stringify(fields)}, not ${JSON.stringify([...ALWAYS, ...model.fields])} in that order`);
        }
        if (model.value !== "fixed_tick_per_unit") {
          const leaked = fields.filter((field) => ONLY_FIXED.includes(field));
          if (leaked.length) fail(`${model.name} shows ${JSON.stringify(leaked)}, which belong to Fixed tick + per unit`);
        }
      }

      // 4. Switching model away and back restores the values typed.
      await chooseModel(page, "all_in_bps");
      for (const [field, value] of Object.entries(TYPED.all_in_bps)) await fill(page, field, value);
      await chooseModel(page, "fixed_tick_per_unit");
      for (const [field, value] of Object.entries(TYPED.fixed_tick_per_unit)) await fill(page, field, value);
      await chooseModel(page, "all_in_bps");
      for (const [field, value] of Object.entries(TYPED.all_in_bps)) {
        const held = await readValue(page, field);
        if (held !== value) fail(`switching back to All-in left ${field} at ${JSON.stringify(held)}, not ${JSON.stringify(value)}`);
      }
      await chooseModel(page, "fixed_tick_per_unit");
      for (const [field, value] of Object.entries(TYPED.fixed_tick_per_unit)) {
        const held = await readValue(page, field);
        if (held !== value) fail(`switching back to Fixed tick left ${field} at ${JSON.stringify(held)}, not ${JSON.stringify(value)}`);
      }

      // 5. A name the library already holds disables Save with the reason beside it.
      await fill(page, "name", DUPLICATE);
      await page.waitForTimeout(120);
      const dupe = await page.evaluate((sel) => {
        const el = document.querySelector(sel);
        const save = el?.querySelector(".cost-dialog-save");
        const reason = el?.querySelector(".cost-dialog-reason");
        return {
          disabled: save?.disabled ?? null,
          reason: (reason?.textContent ?? "").trim(),
          shown: reason ? reason.getBoundingClientRect().height > 0 : false,
        };
      }, dialog);
      if (dupe.disabled !== true) fail(`a duplicate name leaves Save enabled (disabled=${dupe.disabled})`);
      if (!dupe.reason) fail("a duplicate name shows no reason beside Save");
      else if (!dupe.shown) fail(`the duplicate-name reason is not rendered (${JSON.stringify(dupe.reason)})`);

      // 6. A POST answered 400: the message above the buttons, the dialog open, the fields kept.
      await fill(page, "name", REFUSED);
      await page.waitForTimeout(120);
      const before = posted.length;
      await page.locator(`${dialog} .cost-dialog-save`).click();
      await page.locator(`${dialog} .cost-dialog-error`).waitFor({ timeout: 10000 }).catch(() => fail("a refused save rendered no message in the dialog"));
      await page.waitForTimeout(150);
      const refused = await page.evaluate((sel) => {
        const el = document.querySelector(sel);
        const error = el?.querySelector(".cost-dialog-error");
        const foot = el?.querySelector(".cost-dialog-foot");
        return {
          open: Boolean(el),
          message: (error?.textContent ?? "").trim(),
          above: error && foot ? error.getBoundingClientRect().bottom <= foot.getBoundingClientRect().top + 1 : null,
        };
      }, dialog);
      if (!refused.open) fail("the dialog closed on a refused save");
      if (refused.message !== REFUSAL) fail(`the dialog shows ${JSON.stringify(refused.message)}, not the service's ${JSON.stringify(REFUSAL)}`);
      if (refused.above === false) fail("the refusal is not rendered above the dialog's buttons");
      const kept = { ...TYPED.fixed_tick_per_unit, name: REFUSED };
      for (const [field, value] of Object.entries(kept)) {
        const held = await readValue(page, field);
        if (held !== value) fail(`after the refusal ${field} holds ${JSON.stringify(held)}, not ${JSON.stringify(value)}`);
      }
      if (posted.length !== before + 1) fail(`the refused save posted ${posted.length - before} profile(s)`);

      // 7. Save posts the model's fields, closes the dialog, and the profile joins the grid.
      await fill(page, "name", FRESH);
      await page.waitForTimeout(120);
      await page.locator(`${dialog} .cost-dialog-save`).click();
      await page.locator(dialog).waitFor({ state: "hidden", timeout: 10000 }).catch(() => fail("Save did not close the dialog"));
      await page.waitForTimeout(200);
      const body = posted[posted.length - 1];
      if (verbose) console.log(`${mode}: Save posted ${JSON.stringify(body)}`);
      if (!body) fail("Save posted no profile");
      else {
        if (body.model !== "fixed_tick_per_unit") fail(`Save sent model ${JSON.stringify(body.model)}`);
        if (body.name !== FRESH) fail(`Save sent name ${JSON.stringify(body.name)}`);
        for (const [field, value] of Object.entries(TYPED.fixed_tick_per_unit)) {
          if (Number(body[field]) !== Number(value)) fail(`Save sent ${field} = ${JSON.stringify(body[field])}, the dialog showed ${JSON.stringify(value)}`);
        }
        if (body.tick_size <= 0) fail(`Save sent tick_size ${JSON.stringify(body.tick_size)}, which the service refuses`);
      }
      const after = await page.locator(".costs-workspace .cost-profile-card").count();
      if (after !== profiles.length + 1) fail(`after Save the grid shows ${after} card(s), not ${profiles.length + 1}`);
      for (const error of errors) fail(`console error: ${error.split("\n")[0]}`);
    } catch (error) {
      fail(String(error).split("\n")[0]);
    }
    await context.close();
  }
} finally {
  await browser.close();
  served.server.close();
}

if (failures.length) {
  console.error(`costs-check: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(
  `costs-check: ok (web/fixtures/cost-profiles.json with ${profiles.length} profiles, terminal and modern; ${MODELS.map((m) => m.fields.length).join("/")} fields per model, ${DUPLICATE} refused before sending, ${REFUSED} answered 400 and rendered above the buttons with the fields kept, ${FRESH} saved; ${posted.length} profile(s) posted, shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
