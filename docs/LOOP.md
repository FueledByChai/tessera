# Working in loops

How development and research run without a person in the middle of every step. The pieces:

- `CLAUDE.md`: the standing rules any agent follows in this checkout.
- `scripts/check.sh`: the definition of done as a command (fmt, tests, build, example parity,
  web, private checks). Green means a ticket may be committed. CI (`.github/workflows/ci.yml`)
  runs the same script: `--quick` in the engine job and `--web-only` in the web job, so a
  parity break fails the pull request the way it fails the checkout. Run from a worktree it
  resolves the main and private checkouts itself (`--resolve` prints them) and never skips the
  private checks silently.
- `BACKLOG.md`: the ticket queue. Every ticket has a **Done when** line that a test or fixture
  proves. Done is derived from git, not written into the file: `scripts/backlog-status.sh`
  reports every ticket with its state, date, and sha from the commit whose subject carries its
  id, and `--next` names the next ticket to take.
- `scripts/release-notes.sh <from> [<to>]`: what shipped between two refs as Markdown, the
  commits with a ticket id grouped under their backlog section. Tag releases; then
  `scripts/release-notes.sh --archive <tag> <previous-tag> <tag>` moves the shipped tickets, text
  and all, out of `BACKLOG.md` into `CHANGELOG.md` under the tag, so the queue holds only open
  work and the changelog is the record of what shipped when.
- `/next-ticket`: the command that takes the next ticket to done (`.claude/commands/next-ticket.md`).
- `/nightly-studies`: the command that runs the registered study configs and appends to the
  research log in the private repo.

## The development loop

One ticket:

```
/next-ticket
```

Keep going while you are around, checking back every 20 minutes:

```
/loop /next-ticket
```

Unattended: schedule `/next-ticket` as a task in the desktop app (Scheduled tasks) for a nightly
window, with a cap on iterations. Each run ends with a report; read those in the morning together
with `git log` and the changed states in `BACKLOG.md`.

Guardrails that make this safe: every ticket is one commit on a branch or worktree, the loop
never pushes, never touches data, and stops itself when the check fails three times or when the
queue is empty. A ticket that comes back `blocked` is the loop asking for a decision.

## Your part

1. **Write tickets, not prompts.** A ticket is a paragraph of intent plus a **Done when** line
   naming the test or measurable output. If you cannot write that line, the ticket is not ready.
2. **Order the queue.** The loop takes the first eligible `todo`. Move things up or down; add
   `Blocked by` when order matters.
3. **Review outcomes, not progress.** Commits, the backlog states, and the loop's end-of-run
   report. For engine changes, the parity step in `scripts/check.sh` is the first thing to trust.
4. **Push** when a batch is reviewed. The loop never does.
5. **Decide on blocked tickets** and on research findings; those are judgment calls.

## The research loop

Study configs live in `../Tessera-private/research/studies/*.toml`. `/nightly-studies` runs each
one, writes results under the private `research/results/<date>/`, appends a dated entry with the
strongest cells to `../Tessera-private/docs/research-log.md`, and flags anything with |t| above 3
for review. Schedule it nightly once the feature workbench tickets (WB-01 onward) give it
expressions and breakeven costs to rank.

Two cautions carry over from the End-of-Year Dogs study: a bad print looks like a finding, so
the sanitizer and the suspect list stay in front of every study; and a loop only amplifies what it
checks, so a new diagnostic belongs in the check or the ticket's test before it is trusted.

## Refreshing the parity baseline

`examples/expected/` is the frozen output of the two bundled examples. When an engine change is
meant to alter results (a costs model fix, a new guard), run `scripts/check.sh --refresh-baseline`,
commit the new files with the change, and explain the difference in the commit body.
