# Working in loops

How development and research run without a person in the middle of every step. The pieces:

- `AGENTS.md`: the standing rules any agent follows in this checkout, whatever harness runs
  it: the loop section first (the same in every project that uses the loop), then the
  **Project rules**. `CLAUDE.md` is one line that imports it, so Claude Code reads the same
  file Codex, OpenCode, and the rest read.
- `scripts/check.sh`: the definition of done as a command (the proof gate and the coverage
  ratchet from the kit, fmt, tests, build, example parity, web, private checks). Green means
  a ticket may be committed. The gate (HK-23) fails a change under `src/` that brings no
  test, fixture, or check and no `No new test: <reason>` line; the ratchet fails line
  coverage below `coverage-floor.txt`, measured by `scripts/coverage.sh` with cargo-llvm-cov
  in the full check and `--quick`, and a ticket that raises coverage raises the floor with
  `scripts/coverage-ratchet.sh --set`. On a branch that touched nothing under `src/` the
  ratchet prints "skipped, no code change" instead of running its instrumented build
  (HK-28, using the gate's `--code-changed` query); on `main` itself it always runs. CI (`.github/workflows/ci.yml`)
  runs the same script: `--quick` in the engine job and `--web-only` in the web job, so a
  parity break fails the pull request the way it fails the checkout. Run from a worktree it
  resolves the main and private checkouts itself (`--resolve` prints them), builds into the
  cargo target directory every worktree shares (`target-worktrees/` beside the main
  checkout's `target/`, so the 397 dependency crates compile once rather than from cold per
  ticket, and no worktree ever overwrites the binaries the console runs; HK-27), and never
  skips the private checks silently. The web job caches playwright's Chromium keyed on the lock file
  and retries the install once when apt's mirror fails (HK-17), the one flake CI has shown.
- `BACKLOG.md`: the ticket queue. Every ticket has a **Done when** line that a test or fixture
  proves. Done is derived from git, not written into the file: `scripts/backlog-status.sh`
  reports every ticket with its state, date, and sha from the commit whose subject carries its
  id, and `--next` names the next ticket to take.
- `scripts/backlog-status.sh --sprint` and `sprint = [...]` in `.loop.toml` (HK-39): the
  tickets chosen for now, in order. `--next` takes the first ready one of them before file
  order and says so when it falls back; `--sprint` lists them with their states and a
  summary line; the table shows each ticket's sprint position. Choosing a sprint is a
  commit that edits the list, so it is dated and diffable like everything else, and tickets
  carry no sprint state. grill-me asks which new tickets go into the sprint, and
  `scripts/sprint.sh add|remove|set|clear` edits the list (HK-40).
- `scripts/backlog-status.sh --open`, `--show <id>`, and `--stories` (HK-40): the sprint-day
  views. `--open` lists the tickets not done and not in the sprint, grouped by section, with
  blockers and the story each serves (`--section` narrows it); `--show` prints a ticket with
  its state and story, or a story with its tickets; `--stories` lists every story in
  `docs/PRODUCT_BACKLOG.md` (`stories` in `.loop.toml`) with a status derived from git: done
  when every ticket that says `Serves BT-nnn` has landed, open k/n, or unticketed. The
  hand-written `Status:` lines in the product backlog are no longer the record.
- `scripts/release-notes.sh <from> [<to>]`: what shipped between two refs as Markdown, the
  commits with a ticket id grouped under their backlog section. Tag releases; then
  `scripts/release-notes.sh --archive <tag> <previous-tag> <tag>` moves the shipped tickets, text
  and all, out of `BACKLOG.md` into `CHANGELOG.md` under the tag, so the queue holds only open
  work and the changelog is the record of what shipped when.
- `loop/prompts/next-ticket.md`: the prompt that takes the next ticket to done;
  `loop/prompts/grill-me.md`, the one that turns a loose idea into stories, acceptance
  criteria, and tickets; and `loop/prompts/grill-project.md` (HK-32), the first-day
  interview for a project that has nothing yet: five rounds (who and where; the data;
  runtime and deploy; the UI; non-negotiables), each ending in a decision record or a dated
  deferral, then it writes the Project rules section of `AGENTS.md`, the records, the first
  epics, the ticket file, `.loop.toml`, and `scripts/check.sh` from the kit's skeleton for
  the stack (`loop/templates/check/`: Rust, Python, Node or TypeScript, Java with Maven or
  Gradle, Go, or a TODO skeleton), which passes on the empty repository and starts failing
  as code arrives. All three are written against `.loop.toml` and `AGENTS.md` only: no
  project name, build tool, or harness tool appears in them, and `scripts/check.sh` fails if
  one does. `.claude/commands/next-ticket.md`, `grill-me.md`, and `grill-project.md` are
  two-line wrappers that point Claude Code at them (`/next-ticket`, `/grill-me <idea>`,
  `/grill-project <project>`); another harness is pointed at the prompt file itself. Tessera
  already has its rules, so `/grill-project` is for the next repository, run there after the
  kit's `install.sh`.
- The loop kit, [FueledByChai/coding-agent-loop](https://github.com/FueledByChai/coding-agent-loop):
  the source of truth for the loop scripts, the prompts, and the templates any project needs
  to adopt the loop (`AGENTS.md` with an empty Project rules, `loop.toml.example`, a workflow
  skeleton, the branch ruleset, the wrappers, the check skeletons and the CI toolchain steps
  per stack that grill-project splices into the workflow (HK-38), an `install.sh` that also writes
  `CLAUDE.md` as `@AGENTS.md` so Claude Code and Codex read the same file (HK-37), a README that says GitHub
  is the only hosting assumption, and its own `check.sh` that CI runs). It was built here as
  `loop/` (HK-16) and published as `v0.1.0` (HK-19); its history is `loop/`'s. `kit` and
  `kit_ref` in `.loop.toml` name the URL and the tag. `scripts/loop-kit-sync.sh --check`
  (run by `scripts/check.sh`) clones the tag and fails when `scripts/` or `loop/prompts/`
  differ from it, so a loop script is never edited here: change it in the kit, tag a release,
  move `kit_ref`, run `scripts/loop-kit-sync.sh`, and commit the copies. `loop/` here holds
  only the prompts now.
- `scripts/deploy-local.sh`: the deploy loop. Every few minutes (a LaunchAgent from
  `--launchd`, or a scheduled task) it pulls `main` fast-forward when `origin/main` moved,
  builds the engine if `src/`, Cargo, `build.rs`, or the strategies changed and the bundle if
  `web/` did, runs the private checks against the new engine (`scripts/check.sh
  --private-only`, output in `data/ui/private-check.log`), and restarts the console only for
  an engine change whose private checks passed and only when no job or study is running; a
  busy console makes it refuse and wait for the next run, and a failed private check leaves
  the console on its previous build, logs the failure, and repeats it on every idle run until
  a later build passes (HK-18). A restart counts only once the port's listener is the pid it
  started: a health answer alone can come from the old service, which is how two restarts
  went unreported on 2026-09-10 while the console stayed on a stale engine (`lsof` was not on
  the LaunchAgent's PATH and is now called by absolute path; a restart that does not take
  exits 4, marks `data/ui/restart.failed`, and repeats on every idle run; HK-30). The console
  is launched detached from the loop's file descriptors: the launcher subshell gets the log
  and `/dev/null` and execs the console, so the pid file names the console itself and
  nothing of the loop outlives the call (on 2026-09-12 the launcher stayed alive as the
  console's parent holding the pipe of the `$(...)` that waited for it, the run sat for two
  hours, launchd started no new run, and killing the stuck run took the console with it);
  the LaunchAgent from `--launchd` sets `AbandonProcessGroup`, so a stopped or killed loop
  never stops the console (HK-47; the self-test starts a fake service inside `$(...)` and
  fails unless the call returns within five seconds). Each run
  is one line in `data/ui/deploy.log`. So a merged
  pull request reaches the console on the Mac mini without anyone touching it, and a merge
  that breaks the private crate or a private strategy test is caught here, the one place
  that has the private checkout, before the console runs it.
- `scripts/open-ticket-pr.sh`: the hand-off. `--claim` pushes `ticket/<id>` to origin before
  work starts, so a second agent's `backlog-status.sh --next` passes over the ticket; after the
  commit the plain form pushes the branch and opens the pull request whose body is the ticket
  report and whose checks are the CI jobs. Merges are gated by CI (see "Merging" below), so a
  green PR lands on its own; the owner pulls `main` fast-forward and rebuilds; several agents
  can hold several tickets at once as long as their `Blocked by` lines allow it.
- `.loop.toml`: the loop's settings, one flat `[loop]` table read through
  `scripts/loop-config.sh <key>` (`--all` prints the effective values). `default_branch`,
  `backlog`, `check`, `check_fast`, `review_paths`, and `trailer_required` are everything the
  three loop scripts know about this project; a missing file or key falls back to a default
  (`main`, `BACKLOG.md`, `scripts/check.sh`, the check itself, no review paths, trailer
  required). Another project adopts the loop by copying the scripts and writing its own file;
  TOML is only the format and says nothing about the project's language. Each script has a
  `--self-test` that `scripts/check.sh` runs; `open-ticket-pr.sh --self-test` drives the claim,
  the trailer rule, and the review-path policy against a stub `gh` in a fixture repository.

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
- The policy in `scripts/open-ticket-pr.sh`: a PR that leaves the review paths untouched is
  set to auto-merge (rebase) when it is opened, so it lands as soon as both jobs pass on an
  up-to-date branch. A PR that touches one (`review_paths` in `.loop.toml`; here
  `examples/expected/`, the parity baseline) is labelled `needs-review` and waits for the
  owner, because it is the one kind of change the checks cannot judge. The script also refuses
  to open a PR for a commit without the agent trailer while `trailer_required` is on.
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
- `/review-prs`: the command that reviews the open pull requests and posts the `Agent review`
  status each one waits for (see "Reviewing" below). It starts with
  `scripts/open-ticket-pr.sh --update-all` (HK-42), which rebases every open pull request
  that is behind `main`: with several agents working tickets at once, each merge leaves the
  others behind, and the ruleset wants green checks on the exact result, so without this a
  parallel lane stalls until someone runs `--update` by hand.
- `docs/decisions/`: decision records (HK-31), one numbered file per decision from the kit's
  template (Context, Decision, Alternatives, Consequences, what would show it was wrong),
  never edited in place: a change is a new record with `scripts/decisions.sh new "<title>"
  --supersedes NNNN`. `scripts/decisions.sh index` keeps the index; `--check` runs inside
  `scripts/check.sh`. `/grill-me` reads the index before asking anything, asks at least
  three rounds with the last on proofs and edge cases, stops to write a record when a story
  implies a decision none covers, and cites the records in every ticket. A story that
  touches a screen carries a text wireframe (HK-34: a fenced block at most 80 columns wide
  naming the panels, tables, and controls in order) that the owner confirms with the draft;
  the ticket points at it (`Wireframe: BT-nnn`) and its proof matches it, and a canvas or
  HTML mockup goes under `docs/wireframes/` on request. `/grill-project`'s UI round closes
  the same way, with the first screen sketched into its decision record. A prompt cannot be
  unit-tested, so `scripts/prompt-check.sh` (also in the check) fails when a prompt no longer
  carries a phrase that states one of its rules, and a prompt change's PR records one real
  run. The locked product decisions from `docs/PRODUCT_BACKLOG.md` live there as records
  0004 to 0018 (HK-33); the backlog section is a pointer at the index.

## Reviewing

A green build is not a review. Every pull request gets one from an agent before it merges
(HK-24, HK-25), and the merge waits for it the same way it waits for CI.

- **What reviews.** `loop/prompts/review-prs.md`, from the kit, wrapped here as
  `/review-prs`. It takes every open PR whose head commit has no `Agent review` status
  (`scripts/review-status.sh --pending`), reads the ticket the title names and its done
  line, the diff, and the Project rules in `AGENTS.md`, and judges four questions only: is
  the proof present, is the done line met, is a Project rule breached, is there a defect it
  can name with file and line. It posts one review comment with its findings and one commit
  status per head: red only when one of the four failed. Style and preference are comments,
  never a fail. A `backlog/` PR is judged against the backlog format. It never approves,
  merges, or pushes. A new push is a new sha and gets a fresh review.
- **When it runs.** From a scheduled task in the desktop app (Scheduled tasks), every ten
  minutes, running `/review-prs` in this checkout, the same way `/nightly-studies` is
  scheduled. It uses the owner's agent subscription; no API key lives on GitHub. Set the
  schedule up before requiring the status, or nothing merges until the first run.
- **What requires it.** The ruleset on `main` lists `Agent review` among the required status
  checks, beside the two CI jobs (`docs/github/ruleset-main.json` is the full ruleset as it
  should stand; a status posted through the API carries no integration id, so its entry has
  none). Applied once by the owner:

  ```bash
  gh api -X PUT repos/FueledByChai/tessera/rulesets/22655615 --input docs/github/ruleset-main.json
  gh api repos/FueledByChai/tessera/rules/branches/main --jq '.[] | select(.type=="required_status_checks") | .parameters.required_status_checks[].context'
  ```

  With that in force, auto-merge on a green PR waits for the status and fires when it lands.
- **Overriding a wrong red.** The owner posts a green status with the reason, which stays in
  the commit's status history:

  ```bash
  scripts/review-status.sh <sha> pass "override: <reason>"
  ```

  `scripts/review-status.sh --pending` shows what is waiting;
  `gh api repos/FueledByChai/tessera/commits/<sha>/status` shows what was posted.

## What CI does not run, and why there is no self-hosted runner

The public runner runs `scripts/check.sh --quick` and `--web-only`. It cannot run the private
checks (the legacy crate built against this engine, the private strategy tests) or the layout
check against real data, because the private checkout and the market data are not on GitHub.
HK-12 asked whether a self-hosted runner on the Mac mini should close that gap. Decision,
2026-09-09: **no runner**, for these reasons.

- The runner would sit on the machine that holds the market data and the private checkout.
  On a public repository a workflow can be made to run on a self-hosted runner from a fork
  pull request, and GitHub's own guidance is not to use self-hosted runners with public
  repositories for that reason. Gating on `pull_request.head.repo.full_name` and requiring
  approval for outside collaborators narrows the exposure; it does not remove it, and one
  mistake in a workflow edit (an `if:` dropped, a `pull_request_target` trigger) opens the
  machine to whoever opens a PR.
- The gap it would close is already covered at the point where it matters. Every ticket runs
  the full `scripts/check.sh`, private checks included, in the worktree before its commit is
  pushed; the ruleset then reruns the public part on the exact result that lands. The runner
  would turn that convention into enforcement, and that is all it would add.
- What enforcement would buy is small today: one owner, agents that run from this machine, no
  outside contributors. It becomes worth revisiting when PRs arrive from checkouts that do not
  have the private repo beside them, or from people who are not the owner.

What covers the gap instead:

- Before the PR: `scripts/check.sh` in the worktree (the `/next-ticket` prompt requires it, and
  its report says so). A PR whose author skipped the private checks is the review question.
- After the merge: the deploy loop runs the private checks against every new engine build
  and refuses to restart the service when they fail, writing the failure to
  `data/ui/deploy.log` and `data/ui/private-check.log` (HK-18). That catches the case the
  ruleset cannot, on the machine that has the data, without exposing it to anyone.

Revisit HK-12 when either condition above changes; the ticket text keeps the security settings
a runner would need (skip fork PRs, require approval for outside collaborators, keep the private
checkout and data out of logs).

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
