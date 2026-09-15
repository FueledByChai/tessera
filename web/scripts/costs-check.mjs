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
//   7. Save posts the model's fields, closes the dialog, and the new profile joins the table;
// the profiles themselves are one sortable table (UI-13, decision 0024): one row per profile in
// the order the API returned, ENTRY/EXIT/ROUND TRIP/TICK/MIN COMM in the model's own unit with
// a dash where the model has no value, the profile id as the NAME cell's tooltip, every header
// sorting with a second click reversing it, Duplicate opening the dialog under "<name> copy"
// with the whole name selected, and an empty library showing the panel's empty state with New
// profile still in its title bar.
// And a save survives the poll that was already in flight (UI-15): with every GET of
// /api/cost-profiles parked holding the library as it stood when the request arrived, the row a
// save added is still in the table once the parked answer — one profile short of it — is applied.
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
/** A name nothing holds either, saved while a poll of the library is in flight (UI-15). */
const SAVED_LATE = "Desk override · saved during a poll";
/** What the check types, so a model switch can be proven to keep it. */
const TYPED = {
  all_in_bps: { entry_bps: "7.5", exit_bps: "2.5" },
  fixed_tick_per_unit: { tick_size: "0.25", entry_slippage_ticks: "3" },
};
/** The assumption the price line is read against (UI-14): one adverse tick each side at a
 *  $0.01 tick, $0.005 a share each side, no minimum. 1000 shares at $100 is $5 of commission
 *  and $10 of slippage a side, so $30 round trip, which is 3.00 bps of $100,000. */
const PRICED = {
  tick_size: "0.01",
  entry_slippage_ticks: "1",
  exit_slippage_ticks: "1",
  entry_commission_per_unit: "0.005",
  exit_commission_per_unit: "0.005",
  minimum_commission: "0",
};
/** The line each case has to read. A $100,000 round trip is the notional throughout. */
const PRICE = {
  at100: "A $100,000 round trip at $100.00/share costs about $30.00 (3.00 bps)",
  at50: "A $100,000 round trip at $50.00/share costs about $60.00 (6.00 bps)",
  minimum: "A $100,000 round trip at $100.00/share costs about $60.00 (6.00 bps)",
  allIn: "A $100,000 round trip costs about $100.00 (10.00 bps)",
  off: "No modeled cost.",
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
/** The profiles a POST created, as a later GET sees them: the service keeps them, so the
 *  fixture does too — the console polls /api/cost-profiles every three seconds, and a poll
 *  landing just after a save must not wipe the row the save added. */
const created = [];
const json = (body, status = 200) => ({ status, type: "application/json", body: JSON.stringify(body) });
const fixtureApi = async (pathname, req) => {
  if (pathname === "/api/cost-profiles" && req.method === "POST") {
    const body = JSON.parse(await readBody(req));
    posted.push(body);
    if (String(body?.name ?? "").trim() === REFUSED) return json({ error: REFUSAL }, 400);
    const seed = profiles.find((profile) => profile.model === body.model) ?? profiles[0];
    const record = {
      ...seed,
      ...body,
      id: `cost-fixture-${posted.length}`,
      created_at: new Date().toISOString(),
      builtin: false,
    };
    created.push(record);
    return json(record, 201);
  }
  if (pathname === "/api/cost-profiles") return json([...created, ...profiles]);
  if (pathname === "/api/jobs") return json([]);
  if (pathname === "/api/runs") return json([]);
  if (pathname === "/api/automations") return json([]);
  if (pathname === "/api/dashboard") {
    return json({ strategies: [], recent_runs: [], jobs: [], production_strategies: 0, archived_strategies: 0, historical_reports: 0, active_jobs: 0, worker_capacity: 2 });
  }
  return null;
};

/** The nine columns that sort, in wireframe order; a tenth holds the row's Duplicate. */
const COLUMNS = ["name", "origin", "asset_class", "model", "entry", "exit", "round_trip", "tick", "min_comm"];
const DASH = "—";
/** The row Duplicate is proven on: the custom profile, since every one of its values is the
 *  desk's own rather than a zero, so a carried value cannot pass by accident. */
const duplicated = profiles.find((profile) => !profile.builtin) ?? profiles[0];
const dialog = "dialog.cost-dialog[open]";
/** The profiles table: its sort keys, and per row every cell and the NAME cell's tooltip. */
const readTable = (page) =>
  page.evaluate(() => {
    const wrap = document.querySelector(".costs-table");
    if (!wrap) return null;
    return {
      head: [...wrap.querySelectorAll("thead th")].map((th) => th.dataset.sort ?? ""),
      rows: [...wrap.querySelectorAll("tbody tr")].map((tr) => {
        const cells = [...tr.querySelectorAll("td")];
        return {
          cells: cells.map((td) => (td.textContent ?? "").trim()),
          title: cells[0]?.getAttribute("title") ?? null,
        };
      }),
    };
  });
/** The values in one column, top to bottom. */
const column = (table, index) => table.rows.map((row) => row.cells[index]);
/** A money or basis-point cell as a number; a dash as null. */
const asNumber = (text) => {
  if (!text || text === DASH) return null;
  const parsed = Number(text.replace(/[^0-9.-]/g, ""));
  return Number.isFinite(parsed) ? parsed : null;
};
/** Clicks a header and lets the table settle. */
async function sortBy(page, key) {
  await page.locator(`.costs-table th[data-sort="${key}"]`).first().click();
  await page.waitForTimeout(150);
}
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
/** The dialog's price line, as it reads (UI-14). */
const priceLine = (page) =>
  page.evaluate((sel) => (document.querySelector(`${sel} .cost-dialog-price`)?.textContent ?? "").trim(), dialog);
/** Whether the dialog offers a reference price at all: only fixed tick needs one. */
const hasReference = (page) =>
  page.evaluate((sel) => Boolean(document.querySelector(`${sel} [data-field="reference_price"]`)), dialog);

const upstream = (await reachable(consoleOrigin)) ? consoleOrigin : null;
const served = await serveDist(distDir, upstream, fixtureApi);
const browser = await runtime.chromium.launch(LAUNCH);
const failures = [];
try {
  for (const mode of ["terminal", "modern"]) {
    created.length = 0; // each mode starts from the fixture's library, not the last mode's saves
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

      // 1. No create form on the page: every form on it is inside the dialog, and the profiles
      //    are one table row each rather than a grid of cards (UI-13, decision 0024).
      const onPage = await page.evaluate(() => ({
        forms: [...document.querySelectorAll(".costs-workspace form")].filter((f) => !f.closest("dialog")).length,
        inputs: [...document.querySelectorAll(".costs-workspace input, .costs-workspace select")].filter((el) => !el.closest("dialog")).length,
        cards: document.querySelectorAll(".costs-workspace .cost-profile-card").length,
      }));
      if (onPage.forms) fail(`the Costs page still renders ${onPage.forms} create form(s) outside the dialog`);
      if (onPage.inputs) fail(`the Costs page still renders ${onPage.inputs} form control(s) outside the dialog`);
      if (onPage.cards) fail(`the Costs page still renders ${onPage.cards} profile card(s) instead of a table`);

      // The table: one row per profile in the order the API returned, a dash where the model
      // has no value, and the profile id as the NAME cell's tooltip.
      const table = await readTable(page);
      if (!table) fail("the Costs page shows no profiles table");
      else {
        if (verbose) console.log(`${mode}: table ${JSON.stringify(table)}`);
        if (table.head.slice(0, COLUMNS.length).join("|") !== COLUMNS.join("|")) {
          fail(`the table's sortable headers are ${JSON.stringify(table.head)}, not ${JSON.stringify(COLUMNS)}`);
        }
        if (table.rows.length !== profiles.length) fail(`the table shows ${table.rows.length} row(s), the fixture holds ${profiles.length}`);
        profiles.forEach((profile, index) => {
          const row = table.rows[index];
          if (!row) return;
          if (row.cells[0] !== profile.name) fail(`row ${index} is named ${JSON.stringify(row.cells[0])}, the fixture's ${index}th is ${JSON.stringify(profile.name)}`);
          if (row.title !== profile.id) fail(`row ${index}'s NAME tooltip is ${JSON.stringify(row.title)}, not the id ${JSON.stringify(profile.id)}`);
        });
        const off = profiles.findIndex((profile) => profile.model === "none");
        if (off >= 0) {
          const values = table.rows[off]?.cells.slice(4, 9) ?? [];
          if (values.join("|") !== new Array(5).fill(DASH).join("|")) {
            fail(`the costs-off row reads ${JSON.stringify(values)}, not five dashes`);
          }
        } else fail("the fixture holds no costs-off profile to read dashes from");
        const fixed = profiles.findIndex((profile) => profile.model === "fixed_tick_per_unit");
        if (fixed >= 0 && table.rows[fixed]?.cells[7] !== "$0.01") {
          fail(`the fixed-tick row's TICK reads ${JSON.stringify(table.rows[fixed]?.cells[7])}, not "$0.01"`);
        }
      }

      // Sorting: NAME ascending, a second click reversing it; ROUND TRIP the same.
      await sortBy(page, "name");
      const byName = column(await readTable(page), 0);
      const ascending = [...byName].sort((a, b) => a.localeCompare(b));
      if (byName.join("|") !== ascending.join("|")) fail(`clicking NAME left ${JSON.stringify(byName)}, not ${JSON.stringify(ascending)}`);
      await sortBy(page, "name");
      const reversed = column(await readTable(page), 0);
      if (reversed.join("|") !== [...ascending].reverse().join("|")) fail(`clicking NAME again left ${JSON.stringify(reversed)}, not the reverse of ${JSON.stringify(ascending)}`);
      await sortBy(page, "round_trip");
      const byCost = column(await readTable(page), 6).map(asNumber);
      const descending = [...byCost].sort((a, b) => (b ?? -Infinity) - (a ?? -Infinity));
      if (byCost.join("|") !== descending.join("|")) fail(`clicking ROUND TRIP left ${JSON.stringify(byCost)}, not ${JSON.stringify(descending)}`);
      await sortBy(page, "round_trip");
      const ascendingCost = column(await readTable(page), 6).map(asNumber);
      if (ascendingCost.join("|") !== [...descending].reverse().join("|")) fail(`clicking ROUND TRIP again left ${JSON.stringify(ascendingCost)}, not the reverse of ${JSON.stringify(descending)}`);

      // Duplicate on a row opens the dialog carrying that row, under "<name> copy". The row is
      // found by name: the sorts above have moved it off the top.
      const source = duplicated;
      const at = (await readTable(page)).rows.findIndex((row) => row.cells[0] === source.name);
      if (at < 0) fail(`the table shows no row named ${JSON.stringify(source.name)} to duplicate`);
      else {
        await page.locator(".costs-table tbody tr").nth(at).locator(".cost-duplicate").first().click();
        await page.locator(dialog).waitFor({ timeout: 10000 });
        await page.waitForTimeout(150);
        if ((await readValue(page, "name")) !== `${source.name} copy`) {
          fail(`Duplicate opened with the name ${JSON.stringify(await readValue(page, "name"))}, not ${JSON.stringify(`${source.name} copy`)}`);
        }
        if ((await readValue(page, "model")) !== source.model) fail(`Duplicate opened with model ${JSON.stringify(await readValue(page, "model"))}, not ${JSON.stringify(source.model)}`);
        if ((await readValue(page, "asset_class")) !== source.asset_class) fail(`Duplicate opened with asset class ${JSON.stringify(await readValue(page, "asset_class"))}`);
        for (const [field, value] of Object.entries({ tick_size: source.tick_size, entry_slippage_ticks: source.entry_slippage_ticks, exit_slippage_ticks: source.exit_slippage_ticks, entry_commission_per_unit: source.entry_commission_per_unit, exit_commission_per_unit: source.exit_commission_per_unit, minimum_commission: source.minimum_commission })) {
          const held = await readValue(page, field);
          if (Number(held) !== value) fail(`Duplicate opened with ${field} = ${JSON.stringify(held)}, the row says ${JSON.stringify(value)}`);
        }
        const selected = await page.evaluate((sel) => {
          const input = document.querySelector(`${sel} [data-field="name"] input`);
          return input ? input.value.slice(input.selectionStart ?? 0, input.selectionEnd ?? 0) : "";
        }, dialog);
        if (selected !== `${source.name} copy`) fail(`Duplicate selected ${JSON.stringify(selected)}, not the whole copied name`);
        await page.locator(`${dialog} .cost-dialog-cancel`).click();
        await page.locator(dialog).waitFor({ state: "hidden", timeout: 5000 }).catch(() => fail("Cancel did not close the dialog"));
      }

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
      // The saved profile joins the table at once, or with the console's next poll of
      // /api/cost-profiles (every three seconds), which the fixture answers with what was
      // created: a poll in flight when the save landed is answered from the old library.
      const expected = profiles.length + 1;
      let after = await page.locator(".costs-table tbody tr").count();
      for (let attempt = 0; attempt < 20 && after !== expected; attempt += 1) {
        await page.waitForTimeout(250);
        after = await page.locator(".costs-table tbody tr").count();
      }
      if (after !== expected) fail(`after Save the table shows ${after} row(s), not ${expected}`);

      // 8. The dialog prices the assumption it is defining (UI-14).
      await page.locator(".cost-dialog-open").first().click();
      await page.locator(dialog).waitFor({ timeout: 10000 });
      await chooseModel(page, "fixed_tick_per_unit");
      for (const [field, value] of Object.entries(PRICED)) await fill(page, field, value);
      let line = "";
      if (!(await hasReference(page))) fail("fixed tick + per unit offers no reference price to edit");
      else {
        await fill(page, "reference_price", "100");
        await page.waitForTimeout(150);
        line = await priceLine(page);
        if (!line) fail("the dialog shows no price line");
        else if (line !== PRICE.at100) fail(`at $100.00/share the line reads ${JSON.stringify(line)}, not ${JSON.stringify(PRICE.at100)}`);
        await fill(page, "reference_price", "50");
        await page.waitForTimeout(150);
        line = await priceLine(page);
        if (line !== PRICE.at50) fail(`at $50.00/share the line reads ${JSON.stringify(line)}, not ${JSON.stringify(PRICE.at50)}`);
        // A minimum above what the shares would pay is the figure the line uses.
        await fill(page, "reference_price", "100");
        await fill(page, "minimum_commission", "20");
        await page.waitForTimeout(150);
        line = await priceLine(page);
        if (line !== PRICE.minimum) fail(`with a $20 minimum the line reads ${JSON.stringify(line)}, not ${JSON.stringify(PRICE.minimum)}`);
      }
      // All-in basis points needs no price at all, so the control is not rendered for it.
      await chooseModel(page, "all_in_bps");
      await fill(page, "entry_bps", "5");
      await fill(page, "exit_bps", "5");
      await page.waitForTimeout(150);
      line = await priceLine(page);
      if (line !== PRICE.allIn) fail(`all-in at 5 and 5 the line reads ${JSON.stringify(line)}, not ${JSON.stringify(PRICE.allIn)}`);
      if (await hasReference(page)) fail("all-in basis points still offers a reference price it does not use");
      await chooseModel(page, "none");
      await page.waitForTimeout(150);
      line = await priceLine(page);
      if (line !== PRICE.off) fail(`costs off the line reads ${JSON.stringify(line)}, not ${JSON.stringify(PRICE.off)}`);
      if (await hasReference(page)) fail("costs off still offers a reference price it does not use");
      // 9. A created record survives the poll that was already in flight (UI-15). Every GET of
      //    /api/cost-profiles is parked holding the library as it stood when the request
      //    arrived, so the answer the parked poll is finally given is one profile short of what
      //    the save has just added. The check waits for a poll to be in flight before it saves,
      //    and parks every poll after that unanswered so no later, fresh answer can put the row
      //    back and hide the fact that the in-flight one dropped it.
      const polls = { parked: 0, sizes: [], waiters: [], block: false };
      await context.route("**/api/cost-profiles", async (route, request) => {
        try {
          if (request.method() !== "GET") {
            await route.fallback();
            return;
          }
          const snapshot = JSON.stringify([...created, ...profiles]);
          polls.parked += 1;
          polls.sizes.push(JSON.parse(snapshot).length);
          if (polls.block) return; // left unanswered on purpose: no later poll may restore the row
          await new Promise((resolve) => polls.waiters.push(resolve));
          await route.fulfill({ status: 200, contentType: "application/json", body: snapshot });
        } catch {
          // The context closing under a parked request is not a failure of the page.
        }
      });
      for (let attempt = 0; attempt < 100 && !polls.parked; attempt += 1) await page.waitForTimeout(100);
      if (!polls.parked) fail("no poll of /api/cost-profiles was in flight when the profile was saved");
      const beforeLate = await page.locator(".costs-table tbody tr").count();
      await fill(page, "name", SAVED_LATE);
      await page.waitForTimeout(120);
      await page.locator(`${dialog} .cost-dialog-save`).click();
      await page.locator(dialog).waitFor({ state: "hidden", timeout: 10000 }).catch(() => fail("Save did not close the dialog"));
      const savedRow = await page.locator(".costs-table tbody tr").count();
      if (savedRow !== beforeLate + 1) fail(`the save added no row: the table shows ${savedRow} row(s), not ${beforeLate + 1}`);
      polls.block = true;
      for (const resolve of polls.waiters.splice(0)) resolve();
      await page.waitForTimeout(600);
      const afterPoll = await readTable(page);
      if (!polls.sizes.every((size) => size === beforeLate)) {
        fail(`the answer in flight held ${JSON.stringify(polls.sizes)} profile(s), not ${beforeLate} in every one`);
      }
      if (!afterPoll) fail("the Costs page shows no profiles table after the poll landed");
      else {
        if (afterPoll.rows.length !== beforeLate + 1) {
          fail(`once the poll that was in flight was answered the table shows ${afterPoll.rows.length} row(s), not ${beforeLate + 1}`);
        }
        if (!afterPoll.rows.some((row) => row.cells[0] === SAVED_LATE)) {
          fail(`once that poll was answered the table holds no row named ${JSON.stringify(SAVED_LATE)}`);
        }
      }
      await context.unroute("**/api/cost-profiles");
      for (const error of errors) fail(`console error: ${error.split("\n")[0]}`);
    } catch (error) {
      fail(String(error).split("\n")[0]);
    }
    await context.close();
  }
  // An empty library: the panel's empty state, no broken table, New profile still offered.
  for (const mode of ["terminal", "modern"]) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await context.addInitScript((m) => window.localStorage.setItem("bt-display-mode", m), mode);
    await context.route("**/api/cost-profiles", (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: "[]" }),
    );
    const page = await context.newPage();
    const fail = (text) => failures.push(`${mode}: ${text}`);
    try {
      await page.goto(served.url, { waitUntil: "domcontentloaded" });
      await page.locator(".app-shell").waitFor({ timeout: 15000 });
      await page.getByRole("button", { name: /^Costs$/ }).first().click();
      await page.locator(".costs-workspace").waitFor({ timeout: 15000 });
      await page.waitForTimeout(200);
      const empty = await page.evaluate(() => ({
        table: document.querySelectorAll(".costs-table").length,
        state: (document.querySelector(".costs-workspace .empty-state")?.textContent ?? "").trim(),
        open: document.querySelectorAll(".cost-dialog-open").length,
      }));
      if (empty.table) fail("an empty library still renders a profiles table");
      if (!empty.state) fail("an empty library shows no empty state in the panel");
      if (empty.open !== 1) fail(`an empty library offers ${empty.open} New profile button(s), not 1`);
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
  `costs-check: ok (web/fixtures/cost-profiles.json with ${profiles.length} profiles, terminal and modern; ${MODELS.map((m) => m.fields.length).join("/")} fields per model, ${DUPLICATE} refused before sending, ${REFUSED} answered 400 and rendered above the buttons with the fields kept, ${FRESH} saved; ${COLUMNS.length} sortable columns over ${profiles.length} rows with the id as the NAME tooltip, NAME and ROUND TRIP reversed on a second click, ${JSON.stringify(duplicated.name)} duplicated as "${duplicated.name} copy", an empty library keeping New profile; ${JSON.stringify(SAVED_LATE)} saved while a poll was in flight and still in the table once that poll was answered; the line pricing 1 tick and $0.005 a share at $100.00 as ${JSON.stringify(PRICE.at100)}, at $50.00 as ${JSON.stringify(PRICE.at50)}, over a $20 minimum as ${JSON.stringify(PRICE.minimum)}, all-in at 5 and 5 as ${JSON.stringify(PRICE.allIn)}, and costs off as ${JSON.stringify(PRICE.off)}; ${posted.length} profile(s) posted, shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
