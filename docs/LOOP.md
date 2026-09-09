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
- `scripts/deploy-local.sh`: the deploy loop. Every few minutes (a LaunchAgent from
  `--launchd`, or a scheduled task) it pulls `main` fast-forward when `origin/main` moved,
  builds the engine if `src/`, Cargo, `build.rs`, or the strategies changed and the bundle if
  `web/` did, and restarts the console only for an engine change and only when no job or
  study is running; a busy console makes it refuse and wait for the next run. Each run is one
  line in `data/ui/deploy.log`. So a merged pull request reaches the console on the Mac mini
  without anyone touching it.
- `scripts/open-ticket-pr.sh`: the hand-off. `--claim` pushes `ticket/<id>` to origin before
  work starts, so a second agent's `backlog-status.sh --next` passes over the ticket; after the
  commit the plain form pushes the branch and opens the pull request whose body is the ticket
  report and whose checks are the CI jobs. Merges are gated by CI (see "Merging" below), so a
  green PR lands on its own; the owner pulls `main` fast-forward and rebuilds; several agents
  can hold several tickets at once as long as their `Blocked by` lines allow it.

## Merging

`main` accepts nothing but rebased pull requests whose CI is green on the exact result that
lands (HK-10). GitHub's merge queue is not offered on a user-owned repository, so the rules
that give the same guarantee are:

- One ruleset on `main` (repository Settings → Rules, created with the commands below): pull
  requests only, rebase merges only, no force-pushes, no deletion, and both CI jobs (`Engine
  (scripts/check.sh --quick)` and `Web (scripts/check.sh --web-only with a scratch console)`)
  required and **up to date with `main`**. After another PR merges, a branch is behind and
  cannot merge until it is rebased and CI runs again on the rebased commit; that is the queue,
  one PR at a time. `scripts/open-ticket-pr.sh <id> --update` does the rebase on GitHub.
- Repository settings: auto-merge allowed, head branches deleted after a merge, merge commits
  and squash merges off.
- The policy in `scripts/open-ticket-pr.sh`: a PR that leaves `examples/expected` untouched is
  set to auto-merge (rebase) when it is opened, so it lands as soon as both jobs pass on an
  up-to-date branch. A PR that refreshes the parity baseline is labelled `needs-review` and
  waits for the owner, because it is the one kind of change the checks cannot judge.
- What the parity step buys: two PRs can each pass CI and merge cleanly while together changing
  engine results; the up-to-date rule makes the second one rerun the examples on top of the
  first, where the baseline comparison catches it.

The settings, as applied:

```bash
gh api -X PATCH repos/FueledByChai/tessera -F allow_auto_merge=true -F delete_branch_on_merge=true \
  -F allow_merge_commit=false -F allow_squash_merge=false -F allow_rebase_merge=true
gh api -X POST repos/FueledByChai/tessera/rulesets --input ruleset.json   # the ruleset below
gh api repos/FueledByChai/tessera/rules/branches/main                      # what is in force
```

`ruleset.json` targets `~DEFAULT_BRANCH` with `enforcement: active`, no bypass actors, and the
rules `deletion`, `non_fast_forward`, `pull_request` (`required_approving_review_count: 0`,
`allowed_merge_methods: ["rebase"]`), and `required_status_checks`
(`strict_required_status_checks_policy: true`, the two job names above with `integration_id`
15368, GitHub Actions). A `merge_queue` rule was rejected with "Invalid rule 'merge_queue'":
the queue needs an organization-owned repository.

Proof, recorded when the rules went live (ruleset 22655615, 2026-09-09):

- [PR #2](https://github.com/FueledByChai/tessera/pull/2), `probe/parity-break`: `origin/main`
  plus one trade appended to `examples/expected/rsi_mean_reversion/trades.csv`, opened with
  `gh pr create` and `gh pr merge 2 --auto --rebase`. The engine job failed on
  `PARITY BROKEN: rsi_mean_reversion/trades.csv differs from examples/expected`, the web job
  passed, and `gh pr view 2 --json mergeStateStatus` stayed `BLOCKED` with auto-merge
  requested. Closed with `gh pr close 2 --delete-branch`.
- [PR #3](https://github.com/FueledByChai/tessera/pull/3), `probe/up-to-date-merge`:
  `origin/main` plus an empty commit, opened the same way. Both jobs passed,
  `mergeStateStatus` went `CLEAN`, and auto-merge landed it at 15:43:28Z (`merged_by`
  FueledByChai, the account that requested auto-merge) with no one touching it. A manual
  `gh pr merge 3 --rebase` a moment later was refused as already merged.

One rule GitHub adds by default, `require_extra_approval_for_unattributed_changes`, asks for a
review when a commit's author is not a GitHub account; the loop's commits use the owner's
noreply address, so it does not bite. Commits from another identity would.
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
never pushes `main`, hands its commit off through a pull request, never touches data, and stops itself when the check fails three times or when the
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
