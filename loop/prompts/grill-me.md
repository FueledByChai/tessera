Turn a loosely stated feature idea into user stories with acceptance criteria and into tickets
the loop can execute. Interrogate first, draft second, write third. Follow the standing
instructions in `AGENTS.md` (the loop section and the Project rules).

Settings come from `.loop.toml`: `scripts/loop-config.sh backlog` names the ticket file,
`default_branch` the branch to branch from, `check` the full check, `trailer_required`
whether commits sign with an agent trailer, `decisions` the directory of decision records
(`docs/decisions` by default). The Project rules name the product backlog (the file of
stories and long-form acceptance criteria) when the project keeps one; without one, stories
go at the top of the ticket file under a heading of their own.

## 1. Ground yourself before asking anything

Read the decision records: the index in the decisions directory (`docs/decisions/README.md`
by default) and every record that touches the idea. A decision that has a record is settled;
never ask about it, cite it by number. Then read the product backlog (every epic that
touches the idea), the ticket file (the protocol and the tickets in the sections the idea
touches), and run `scripts/backlog-status.sh` so you know what has landed. Skim the code and
docs the idea would change, using the Layout in the Project rules to find them, so every
question you ask is one the repository cannot answer.

## 2. Grill

Ask in rounds: at most four questions per round, each with a recommended default first, in
whatever way the harness offers for asking the owner (a structured question tool when there is
one, otherwise numbered questions in chat). Ask only questions whose answer changes a story, a
done line, or a decision. Ask at least three rounds, more when an answer changes a story; the
last round is proofs and edge cases only. No story is drafted without a named proof: if none
comes to mind for a story, ask again until one does, or drop the story and say so.

Cover, in roughly this order, skipping what the idea, the records, or the repository already
settle:

- **Who and why.** Which user and what they do today instead. What would make them stop
  using it.
- **Edges of scope.** What is explicitly out. Which existing page, endpoint, command, or module
  it changes, and which it must leave alone.
- **Inputs and data.** What it reads; what happens when that is missing, partial, or wrong.
  Whether anything private is involved that the Project rules keep out of this repository.
- **Where it shows up.** Which surface: a page and panel, an endpoint, command output, or a
  file. For a page: which of the UI conventions in the Project rules apply, and what the
  panel holds in what order. Sketch that as a text wireframe in the next round's restatement,
  so the owner corrects a picture rather than a paragraph.
- **Decisions.** A story that implies a decision no record covers (a store, a protocol, a
  library, a boundary, a rule) does not get to guess: stop, ask the owner the decision as its
  own question with the alternatives named, and write the record in this session
  (`scripts/decisions.sh new "<title>"`, then fill its Context, Decision, Alternatives,
  Consequences, and what would show it was wrong). A story that contradicts a record stops
  for "which wins"; when the decision changes, write a superseding record
  (`scripts/decisions.sh new "<title>" --supersedes NNNN`) rather than editing the old one.
  Every ticket cites the records it rests on.
- **Failure and guardrails.** What must refuse rather than guess. What a wrong answer would
  look like in the output.
- **Proof** (the last round). For each candidate story: what test, fixture, self-test, or
  measurable output would convince the owner it is done, and which edge cases that proof
  must cover. If nothing testable comes to mind, the story is not ready.
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
what stays fixed. Mention the story it serves and the decision records it rests on
("Decisions: 0007, 0012").>
**Done when:** <the test, fixture, script self-test, or measurable output that ships in the same
commit, by name: a test function, a check script, a fixture, a line a command prints, a value
an endpoint returns>.
```

A ticket is one commit's worth of work for one agent, testable on its own with the full check.
Split anything larger. Put `Blocked by` only where the work cannot start earlier. Do not add
`todo` or any other state; the heading carries no state until someone claims it.

**Wireframes.** A story that touches a screen carries, after its acceptance criteria, a
fenced text wireframe at most 80 columns wide that names the panels, tables, and controls and
their order, top to bottom and left to right, with what the story adds or changes marked.
The owner confirms it with the draft. The ticket points at it by story id (`Wireframe:
BT-nnn`) rather than describing the layout again, and its done line's fixture or check
matches the wireframe. When the owner asks for a mockup, or the screen is new rather than
changed, make one with the harness's design canvas when it has one and otherwise as a
single HTML file, save it beside the product backlog (`docs/wireframes/<story id>.html`),
and link it from the story and the ticket.

Ask the owner to confirm the draft, and apply their edits, before going on.

## 4. Write and hand off

Never write to the default branch. From the main checkout:

1. `git checkout -b backlog/<short-slug>` from the default branch.
2. Insert the stories and tickets. Keep the files' ordering (epics by letter, tickets by id).
   If a story replaces or narrows an existing one, edit that story's status line rather than
   adding a duplicate. The decision records written during the session go in the same
   commit, with the index (`scripts/decisions.sh index`).
3. Commit with the subject `Backlog: <XX-nn..XX-mm> <one-line summary>` and, when the config
   requires it, a `Co-Authored-By: <agent> <email>` trailer naming the agent and model.
4. `git push -u origin backlog/<short-slug>` and open the PR with `gh pr create --fill`, then
   `gh pr merge --auto --rebase`; CI on a docs-only change is quick and the owner can merge or
   wait for auto-merge.
5. Report: the PR URL, the story ids and ticket ids added, the decision records written or
   superseded, what the first next-ticket run will pick up, and any question the owner
   deferred (record those as a `blocked <question>` claim on the ticket that needs the answer).

Rules: this prompt changes only the product backlog, the ticket file, the decision
records, and the wireframe files. It never edits code or anything the Project rules say never to touch, and never
copies private details into a public backlog.
