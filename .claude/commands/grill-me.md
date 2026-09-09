Turn a loosely stated feature idea into user stories with acceptance criteria and into tickets
the loop can execute. Interrogate first, draft second, write third. Follow CLAUDE.md.

The idea: $ARGUMENTS

## 1. Ground yourself before asking anything

Read `docs/PRODUCT_BACKLOG.md` (the **Locked product decisions** and every epic that touches the
idea), `BACKLOG.md` (the protocol and the tickets in the sections the idea touches), and run
`scripts/backlog-status.sh` so you know what has landed. Skim the code and docs the idea would
change (`src/`, `src/bin/tessera_ui.rs`, `web/app/page.tsx`, `docs/*.md`) so every question you
ask is one the repo cannot answer. Never ask something the owner already decided in the locked
list or in an existing story; cite it instead.

## 2. Grill

Ask in rounds. Use the AskUserQuestion tool when it is available (at most four questions per
round, each with a recommended default first); otherwise ask in chat, numbered. Ask only
questions whose answer changes a story or a done line. Keep going until you can name every
story's proof without guessing; three or four rounds is typical, one is a sign you did not push.

Cover, in roughly this order, skipping what the idea or the repo already settles:

- **Who and why.** Which user (strategy author, researcher, operator, maintainer) and what they
  do today instead. What would make them stop using it.
- **Edges of scope.** What is explicitly out. Which existing page, endpoint, CLI subcommand, or
  strategy it changes, and which it must leave alone.
- **Inputs and data.** Which series, bar resolutions, universes, or artifacts it reads; what
  happens when they are missing, partial, off-calendar, or survivorship-biased. Whether anything
  private (strategies, configs, data paths) is involved, which keeps it out of this repo.
- **Where it shows up.** Console page and panel, JSON endpoint, CLI output, or artifact file.
  For the console: which of the terminal-look rules apply (tables, charts, tabs or caps, fits a
  13-inch laptop).
- **Locked decisions.** Any tension with the locked product decisions or with a story already
  Ready or In progress; if there is one, say so and ask which wins.
- **Failure and guardrails.** What must refuse rather than guess (broker guards, sanitation,
  provenance). What a wrong answer would look like in the numbers.
- **Proof.** For each candidate story: what test, fixture, self-test, or measurable output would
  convince the owner it is done. If nothing testable comes to mind, the story is not ready;
  ask again until it is.
- **Order and dependencies.** What has to exist first; what could ship on its own.

Push back on vague answers ("it should just work", "like the other page") with a concrete
alternative to accept or reject. Restate the owner's answers in your own words at the start of
the next round so misreadings surface early.

## 3. Draft

Produce two artifacts and show both in chat before writing anything.

**Product stories** for `docs/PRODUCT_BACKLOG.md`, in its exact format under the matching epic
(or a new epic with the next letter and a new `BT-` hundred block):

```
### BT-nnn — <title>

**Status:** Proposed  
**User story:** As a <user>, I want <capability> so that <outcome>.

**Acceptance criteria:**

- <observable, testable statement>
- ...
```

Each criterion is a statement someone could check without reading the code. No "works
correctly", "is fast", "handles errors"; say what is shown, refused, frozen, or measured.

**Executable tickets** for `BACKLOG.md`, one per shippable slice, in its exact format under the
matching section (or a new `## <section>` with a new two-letter prefix), ids continuing that
prefix's sequence (`grep -o '^### XX-[0-9]*' BACKLOG.md | sort | tail -1`):

```
### XX-nn <title> — Blocked by XX-mm
<One paragraph: what is wrong or missing today, with file and function names; what changes;
what stays fixed. Mention the BT story it serves.>
**Done when:** <the test, fixture, script self-test, or measurable output that ships in the same
commit, by name: a test function in `src/...`, a check in `web/scripts/...-check.mjs`, a
fixture under `web/fixtures/`, a line the CLI prints, a value an endpoint returns>.
```

A ticket is one commit's worth of work for one agent, testable on its own with `scripts/check.sh`.
Split anything larger. Put `Blocked by` only where the work cannot start earlier. Do not add
`todo` or any other state; the heading carries no state until someone claims it.

Ask the owner to confirm the draft, and apply their edits, before going on.

## 4. Write and hand off

Never write to `main`. From the main checkout:

1. `git checkout -b backlog/<short-slug>` from `main`.
2. Insert the stories and tickets. Keep the files' ordering (epics by letter, tickets by id).
   If a story replaces or narrows an existing one, edit that story's status line rather than
   adding a duplicate.
3. Commit with the subject `Backlog: <XX-nn..XX-mm> <one-line summary>` and the
   `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` trailer.
4. `git push -u origin backlog/<short-slug>` and open the PR with `gh pr create --fill`, then
   `gh pr merge --auto --rebase`; CI on a docs-only change is quick and the owner can merge or
   wait for auto-merge.
5. Report: the PR URL, the story ids and ticket ids added, what the first `/next-ticket` will
   pick up, and any question the owner deferred (record those as a `blocked <question>` claim on
   the ticket that needs the answer).

Rules: this command changes only `docs/PRODUCT_BACKLOG.md` and `BACKLOG.md`. It never edits code,
market data, `artifacts/`, or the private checkout, and never copies private strategy, config,
or data-path details into the public backlog.
