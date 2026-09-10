Turn a loosely stated feature idea into user stories with acceptance criteria and into tickets
the loop can execute. Interrogate first, draft second, write third. Follow the standing
instructions in `AGENTS.md` (the loop section and the Project rules).

Settings come from `.loop.toml`: `scripts/loop-config.sh backlog` names the ticket file,
`default_branch` the branch to branch from, `check` the full check, `trailer_required`
whether commits sign with an agent trailer. The Project rules name the product backlog (the
file of stories, locked decisions, and long-form acceptance criteria) when the project keeps
one; without one, stories go at the top of the ticket file under a heading of their own.

## 1. Ground yourself before asking anything

Read the product backlog (its locked decisions and every epic that touches the idea), the
ticket file (the protocol and the tickets in the sections the idea touches), and run
`scripts/backlog-status.sh` so you know what has landed. Skim the code and docs the idea would
change, using the Layout in the Project rules to find them, so every question you ask is one
the repository cannot answer. Never ask something the owner already decided in a locked
decision or an existing story; cite it instead.

## 2. Grill

Ask in rounds: at most four questions per round, each with a recommended default first, in
whatever way the harness offers for asking the owner (a structured question tool when there is
one, otherwise numbered questions in chat). Ask only questions whose answer changes a story or
a done line. Keep going until you can name every story's proof without guessing; three or four
rounds is typical, one is a sign you did not push.

Cover, in roughly this order, skipping what the idea or the repository already settles:

- **Who and why.** Which user and what they do today instead. What would make them stop
  using it.
- **Edges of scope.** What is explicitly out. Which existing page, endpoint, command, or module
  it changes, and which it must leave alone.
- **Inputs and data.** What it reads; what happens when that is missing, partial, or wrong.
  Whether anything private is involved that the Project rules keep out of this repository.
- **Where it shows up.** Which surface: a page and panel, an endpoint, command output, or a
  file. For a page: which of the UI conventions in the Project rules apply.
- **Locked decisions.** Any tension with a locked decision or with a story already in
  progress; if there is one, say so and ask which wins.
- **Failure and guardrails.** What must refuse rather than guess. What a wrong answer would
  look like in the output.
- **Proof.** For each candidate story: what test, fixture, self-test, or measurable output would
  convince the owner it is done. If nothing testable comes to mind, the story is not ready;
  ask again until it is.
- **Order and dependencies.** What has to exist first; what could ship on its own.

Push back on vague answers ("it should just work", "like the other page") with a concrete
alternative to accept or reject. Restate the owner's answers in your own words at the start of
the next round so misreadings surface early.

## 3. Draft

Produce two artifacts and show both in chat before writing anything.

**Product stories**, in the product backlog's exact format under the matching epic (or a new
epic with the next letter and a new hundred block of its story prefix):

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

**Executable tickets** for the ticket file, one per shippable slice, in its exact format under
the matching section (or a new `## <section>` with a new two-letter prefix), ids continuing
that prefix's sequence (`grep -o '^### XX-[0-9]*' <ticket file> | sort | tail -1`):

```
### XX-nn <title> — Blocked by XX-mm
<One paragraph: what is wrong or missing today, with file and function names; what changes;
what stays fixed. Mention the story it serves.>
**Done when:** <the test, fixture, script self-test, or measurable output that ships in the same
commit, by name: a test function, a check script, a fixture, a line a command prints, a value
an endpoint returns>.
```

A ticket is one commit's worth of work for one agent, testable on its own with the full check.
Split anything larger. Put `Blocked by` only where the work cannot start earlier. Do not add
`todo` or any other state; the heading carries no state until someone claims it.

Ask the owner to confirm the draft, and apply their edits, before going on.

## 4. Write and hand off

Never write to the default branch. From the main checkout:

1. `git checkout -b backlog/<short-slug>` from the default branch.
2. Insert the stories and tickets. Keep the files' ordering (epics by letter, tickets by id).
   If a story replaces or narrows an existing one, edit that story's status line rather than
   adding a duplicate.
3. Commit with the subject `Backlog: <XX-nn..XX-mm> <one-line summary>` and, when the config
   requires it, a `Co-Authored-By: <agent> <email>` trailer naming the agent and model.
4. `git push -u origin backlog/<short-slug>` and open the PR with `gh pr create --fill`, then
   `gh pr merge --auto --rebase`; CI on a docs-only change is quick and the owner can merge or
   wait for auto-merge.
5. Report: the PR URL, the story ids and ticket ids added, what the first next-ticket run will
   pick up, and any question the owner deferred (record those as a `blocked <question>` claim on
   the ticket that needs the answer).

Rules: this prompt changes only the product backlog and the ticket file. It never edits code
or anything the Project rules say never to touch, and never copies private details into a
public backlog.
