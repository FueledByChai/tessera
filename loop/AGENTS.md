# Standing instructions for agents

This file is the contract for any agent working in this checkout, whatever harness or model
runs it. It replaces instructions that would otherwise have to be repeated in chat. The first
part is the ticket loop, which is the same in every project that uses it; the **Project rules**
below it are this project's own and are what the loop prompts mean when they say "Project rules".

## The loop

- **Settings.** `.loop.toml` holds everything the loop knows about this project:
  `default_branch`, `backlog` (the ticket file), `check` (the full check), `check_fast` (the
  check to run while iterating), `review_paths` (changes that need a human review),
  `trailer_required`, and `kit` (where the loop kit lives). `scripts/loop-config.sh --all`
  prints the effective values. Prompts and scripts read them from there; they never hard-code
  a branch, a path, or a build command.
- **Tickets.** The backlog is a list of tickets, each a paragraph of intent plus a **Done when**
  line naming the test, fixture, or measurable output that proves it. Git is the record of
  done: a ticket is done when a commit whose subject starts with its id is on the default
  branch. `scripts/backlog-status.sh` derives every ticket's state from the commits and
  `--next` names the first `todo` whose `Blocked by` tickets have landed. The backlog file
  carries only claims: `doing` while someone works a ticket, `blocked <reason>` when it needs
  a decision. Clear the `doing` claim in the ticket's own commit and never write a done line.
  Anything discovered while working goes in as a new ticket, not into the current one.
- **Claims and hand-off.** Before work starts, `scripts/open-ticket-pr.sh <id> --claim` pushes
  `ticket/<id>` to origin; `backlog-status.sh --next` passes over claimed ids, so several
  agents can hold several tickets. After the commit, `scripts/open-ticket-pr.sh <id>` pushes
  the branch and opens the pull request; a green PR up to date with the default branch merges
  on its own, one that touches a review path is labelled `needs-review` and waits for the
  owner. Never push the default branch. Never force-push. Never rewrite its history.
- **Commits.** One commit per ticket. The subject starts with the ticket id (`AB-12: ...`); the
  body says what changed and how the done line is proven; the message ends with a
  `Co-Authored-By: <agent> <email>` trailer naming the agent and model that did the work when
  `trailer_required` is on (the PR script refuses a commit without one).
- **Definition of done.** The full check (`check` in `.loop.toml`) passes, and the commit
  includes the test or fixture that proves the ticket's done line. Run the fast check while
  iterating and the full check before committing. CI runs the same script; there are no
  separate hand-written CI steps to keep in sync.
- **Isolation.** Prefer an isolated worktree per ticket. The check script knows how to run
  from one (see the project rules for what it resolves).
- **Releases.** Tag them: `scripts/release-notes.sh <from> <to>` lists what shipped, and
  `--archive <tag>` moves the shipped tickets out of the backlog into `CHANGELOG.md`.
- **Prompts.** `loop/prompts/next-ticket.md` takes the next ticket to done;
  `loop/prompts/grill-me.md` turns a loose idea into stories, acceptance criteria, and
  tickets. A harness with slash commands wraps them in its command directory; any other
  agent is pointed at the prompt file directly.
- **The kit.** The scripts and prompts are copies from the loop kit named by `kit` in
  `.loop.toml`; `scripts/loop-kit-sync.sh --check` fails when they drift, and
  `scripts/loop-kit-sync.sh` brings them up to the kit's tag. Change them in the kit, not here.

## Project rules

(What this project is; its layout; how to build, run, and restart it; what the check covers
and what it resolves from a worktree; what never to touch; conventions the prompts should
follow; docs to keep current.)
