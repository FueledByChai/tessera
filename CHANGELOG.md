# Changelog

What shipped, by release: the commits that carry a ticket id between two tags, with the
ticket text as it stood when it left BACKLOG.md (scripts/release-notes.sh --archive).

The `housekeeping` entry is the `HK-` work that had already shipped when this file was created:
the commits proving those tickets are in this repository's history, so their bodies are archived
here rather than re-listed in BACKLOG.md.

## housekeeping — 2026-09-12 (f4be87d..origin/main)

### Housekeeping (HK)

- **HK-01** Required symbols from the manifest — 2026-09-06 · 1f296ae
- **HK-02** Sanitation counts on the run page — 2026-09-08 · 13da7fc (sanitation counts on the run page from a sanitation.json sidecar)
- **HK-03** CI runs scripts/check.sh — 2026-09-08 · 0c43c77 (CI runs scripts/check.sh (--quick engine job, --web-only web job))
- **HK-04** check.sh works from a worktree — 2026-09-08 · 5912ef4 (check.sh resolves the main and private checkouts from a worktree)
- **HK-05** Ticket state derived from git — 2026-09-08 · 0fbce38
- **HK-06** Release notes from commits and a changelog archive — 2026-09-08 · 8ab4cb8
- **HK-07** Layout check runs in CI — 2026-09-08 · 1149faf (layout check runs in CI against a scratch console)
- **HK-08** Layout check skips pages the console cannot supply — 2026-09-08 · 14a1982
- **HK-09** The loop hands off through pull requests — 2026-09-09 · a3dfcfb
- **HK-10** Merges gated by CI: branch protection and the merge queue — 2026-09-09 · b693d8f (merges gated by CI through branch rules and auto-merge)
- **HK-11** Local deploy loop — 2026-09-09 · cd5bf50
- **HK-13** web/node_modules lives outside iCloud — 2026-09-09 · c97c3ee (the checkout lives in ~/Code, outside iCloud)
- **HK-12** Self-hosted runner for the full check — 2026-09-09 · 5a47c36 (no self-hosted runner; the deploy loop guards the private checks instead)
- **HK-14** Loop settings live in `.loop.toml`, not in the scripts — 2026-09-09 · 1d9b250 (loop settings live in .loop.toml, read through scripts/loop-config.sh)
- **HK-15** Instructions and prompts any coding agent can read — 2026-09-09 · 3fa9509 (standing instructions in AGENTS.md; loop prompts generic under loop/prompts)
- **HK-16** The loop kit is its own repository — 2026-09-10 · 961a658 (the loop kit, laid out as its own repository under loop/, with install and sync)
- **HK-19** The loop kit is published and this checkout consumes it by tag — 2026-09-10 · 6b41ab8 (the loop kit is published as coding-agent-loop v0.1.0 and consumed by tag)
- **HK-17** CI installs Playwright's browser without depending on the apt mirror — 2026-09-10 · 14a6d34 (CI caches playwright's Chromium and retries the install when apt's mirror fails)
- **HK-18** Deploy loop runs the private checks before it restarts the service — 2026-09-10 · aedb9a6 (the deploy loop runs the private checks before it restarts the service)
- **HK-20** backlog-status --next judges done against origin, not a lagging local main — 2026-09-10 · ea1fe80 (backlog-status judges done against origin's main; kit v0.2.0)
- **HK-21** Proof gate in the kit: code changes must touch a proof — 2026-09-10 · c79ed0d (proof gate in the kit (coding-agent-loop v0.3.0), synced here)
- **HK-22** Coverage ratchet in the kit: a floor that only rises — 2026-09-10 · c36ecca (coverage ratchet in the kit (coding-agent-loop v0.4.0), synced here)
- **HK-23** Tessera measures coverage with cargo-llvm-cov and gates on the floor — 2026-09-10 · 6ce9259 (the proof gate and the coverage ratchet gate this checkout; cargo-llvm-cov measures it)
- **HK-24** Agent review prompt and status in the kit — 2026-09-10 · 78fad60 (agent review prompt and status in the kit (coding-agent-loop v0.5.0), synced here)
- **HK-25** Tessera PRs wait for the agent review — 2026-09-10 · 1adecff (the agent review is wired in; the ruleset change and the schedule are the owner's)
- **HK-26** Coverage ratchet tolerates measurement jitter — 2026-09-10 · c95bbd2 (the coverage ratchet tolerates jitter (coding-agent-loop v0.6.0); slack 0.2 here)
- **HK-27** One cargo target directory for the main checkout and every worktree — 2026-09-10 · 94d3e5d (one cargo target directory shared by every worktree)
- **HK-28** The coverage ratchet skips itself when no code changed — 2026-09-10 · 5556e9c (the coverage ratchet skips its instrumented build when no code changed)
- **HK-29** The proof gate sees the working tree, not only commits — 2026-09-10 · 73fe30f (the proof gate judges the working tree (coding-agent-loop v0.8.0), synced here)
- **HK-30** The deploy loop sees the listener under launchd and confirms the restart — 2026-09-10 · 770d7d6 (the deploy loop sees the listener under launchd and reports a restart only when it took)
- **HK-31** Decision records, and grill-me that reads, writes, and respects them — 2026-09-11 · c7dc6ae (decision records, and grill-me that reads, writes, and respects them (kit v0.9.0))
- **HK-32** grill-project: the first-day interview that leaves rules, decisions, epics, and a check behind — 2026-09-11 · 3892b56 (grill-project, the first-day interview (coding-agent-loop v0.10.0), synced here)
- **HK-35** The kit sync replaces files atomically, so it cannot break itself mid-run — 2026-09-11 · eb9efef (the kit sync replaces files by rename and reruns its new copy (coding-agent-loop v0.11.0), synced here)
- **HK-36** grill-project asks for the project's purpose as its own first question — 2026-09-11 · aa8c1a6 (grill-project opens with the purpose, in a sentence (coding-agent-loop v0.12.0), synced here)
- **HK-34** grill-me sketches the screen a story touches — 2026-09-11 · d6a9278 (grill-me and grill-project sketch the screen a story touches (coding-agent-loop v0.13.0), synced here)
- **HK-37** The kit's installer writes CLAUDE.md so Claude Code loads AGENTS.md — 2026-09-11 · 9ca1a6f (the kit's installer writes CLAUDE.md as @AGENTS.md (coding-agent-loop v0.14.0), synced here)
- **HK-38** grill-project writes the workflow's toolchain step from the stack table — 2026-09-11 · 31588e4 (grill-project writes the workflow's toolchain steps (coding-agent-loop v0.15.0), synced here)
- **HK-39** A sprint list in .loop.toml steers --next — 2026-09-11 · 9bd528f (a sprint list in .loop.toml steers --next (coding-agent-loop v0.16.0), synced here)
- **HK-40** Sprint-day views: open tickets, a ticket or story in full, and derived story status — 2026-09-11 · 4e9c865 (sprint-day views and a sprint helper (coding-agent-loop v0.17.0), synced here)
- **HK-42** The review pass rebases the open pull requests a merge left behind — 2026-09-11 · de70e0d (the review pass rebases the open pull requests a merge left behind (coding-agent-loop v0.18.0), synced here)
- **HK-33** Tessera's locked product decisions become records — 2026-09-11 · 4f6bc26 (the locked product decisions become records 0004 to 0018)
- **HK-47** The deploy loop returns from a restart, and a killed loop does not take the console down — 2026-09-12 · 5c5c0ab (the deploy loop returns from a restart, and a killed loop keeps the console up)
- **HK-44** The deploy loop keys on the build it last deployed, not on the pull delta — 2026-09-12 · 98b0cc6

### Housekeeping (HK): archived tickets

#### HK-01 Required symbols from the manifest — 2026-09-06 · 1f296ae
A manifest can declare symbols the runner always appends (End-of-Year Dogs needs `IWM.US` for the
hedge even when the form lists only `universe:stocks`).
**Done when:** a test shows the appended symbol present in the plan when omitted from the form,
and the Dogs manifest declares it.

#### HK-02 Sanitation counts on the run page — 2026-09-08 · 13da7fc
Show dropped off-calendar rows, dropped spikes, and skipped symbols from the run log in the
Overview tab, with the skipped symbols listed on the Symbols tab.
**Done when:** a run over `examples/data` with an injected holiday row shows the count in the UI.

#### HK-03 CI runs scripts/check.sh — 2026-09-08 · 0c43c77
Replace the hand-written CI steps with `scripts/check.sh --quick` plus the web job.
**Done when:** the workflow file calls the script and a deliberate parity break fails CI locally
with `act` or in a PR.

#### HK-04 check.sh works from a worktree — 2026-09-08 · 5912ef4
From `.claude/worktrees/<name>` the script cannot find `../Tessera-private` (so the private checks
silently skip), a fresh worktree has no `local.toml` (so private strategies do not compile in), and
the private legacy crate builds against the main checkout's engine, so a private strategy that uses
a new SDK method fails the private check until the public commit reaches `main`. Found while
working HK-01 in a worktree.
**Done when:** `scripts/check.sh` run from a worktree resolves the private checkout through the
repository's common git dir (or `TESSERA_PRIVATE_ROOT`), copies or points at the main `local.toml`,
and a test run from a worktree reports the private checks as run, not skipped.

#### HK-05 Ticket state derived from git — 2026-09-08 · 0fbce38
Every ticket edits this file to write `done <date> <sha>`, which is the one line two parallel
loops always collide on, and the sha cannot be known before the commit exists. Make git the
record: a ticket is `done` when a commit whose subject starts with its id is on `main`. This
file keeps only `todo`, `doing`, and `blocked` (`doing` stays as the claim; the loop clears it in
the ticket's commit). Add `scripts/backlog-status.sh` that prints every ticket with its derived
state, date, and short sha, and update `CLAUDE.md` and `.claude/commands/next-ticket.md` so the
loop stops writing `done` lines and picks the first `todo` whose blockers have a commit on `main`.
**Done when:** the status script lists HK-01 and WB-02 as done with their dates and shas while
their lines here carry no state, and a test fixture repo in the script's tests shows a `doing`
ticket with a landed commit reported as done.

#### HK-06 Release notes from commits and a changelog archive — 2026-09-08 · 8ab4cb8 — Blocked by HK-05
`scripts/release-notes.sh <from-ref> [<to-ref>]` lists commits whose subject starts with a
ticket id, grouped by prefix (WB, HK) with date and short sha, as Markdown. `--archive <tag>`
moves the completed tickets' entries out of this file into `CHANGELOG.md` under the tag's
heading, so the queue holds only open work and the changelog is the record of what shipped
when. Tag releases; the notes are the diff between tags.
**Done when:** the script over `3022548..main` lists HK-01 and WB-02 under their sections with
dates; `--archive` on a scratch copy produces a `CHANGELOG.md` containing them and a
`BACKLOG.md` without them; `scripts/check.sh --quick` runs the script's self-test.

#### HK-07 Layout check runs in CI — 2026-09-08 · 1149faf — Blocked by HK-03
`web/scripts/layout-check.mjs` (UI-03) skips when no Chromium is resolvable and when no console
answers on 8787, so on CI it currently proves nothing. Add `playwright` as a web devDependency,
install its Chromium in the workflow, and run the check against the built bundle with the
bundled example data (a scratch `TESSERA_ROOT` with `examples/`, one run, one study fixture).
**Done when:** the CI job logs `layout-check: ok` and a deliberate bare-`fr` regression fails it.

#### HK-08 Layout check skips pages the console cannot supply — 2026-09-08 · 14a1982
`web/scripts/layout-check.mjs` reports eight "could not open" failures when the console on 8787
has no runs or strategies (a scratch instance on an empty catalog, seen during WB-09), so a
check run beside a scratch service fails for reasons unrelated to the layout. Pages the console
cannot supply should be listed as skipped, like a missing console, while the rest are measured.
**Done when:** the check run against an empty catalog exits 0 with the run and strategy pages
listed as skipped and the studies page measured.

#### HK-09 The loop hands off through pull requests — 2026-09-09 · a3dfcfb
Today a ticket ends in a fast-forward merge and a service restart only the owner can do, so
tickets run one at a time even when nothing blocks them. An agent should push a `ticket/<id>`
branch (never `main`) and open a pull request whose body is the ticket report (id, what changed,
how the done line is proven, what the owner should look at), and the claim on a ticket should
be that remote branch rather than a `doing` line that only exists in a worktree:
`scripts/backlog-status.sh --next` skips ids with a `ticket/<id>` branch on `origin`, and
`scripts/open-ticket-pr.sh <id>` pushes the branch and opens the PR with `gh`. `CLAUDE.md` and
`.claude/commands/next-ticket.md` change from "never push" to "never push `main`". Needs `gh`
installed and authenticated on this machine.
**Done when:** the status script's self-test shows a ticket with a remote `ticket/<id>` branch
skipped by `--next`, and a ticket worked from a worktree ends with an open PR whose body carries
the report and whose checks are the CI jobs.

#### HK-10 Merges gated by CI: branch protection and the merge queue — 2026-09-09 · b693d8f — Blocked by HK-09
With several PRs open at once a merge must be tested as the result it produces, not as the
branch on its own. Turn on branch protection for `main` (the engine and web CI jobs as required
status checks, no direct pushes) and the GitHub merge queue, so each PR merges only after CI
passes on `main` plus the PRs ahead of it; the parity step is what catches two clean merges that
change engine results together. Auto-merge on green for PRs that leave `examples/expected`
untouched; a PR that refreshes the parity baseline requires a review. Document the policy and
the settings in `docs/LOOP.md`.
**Done when:** `gh api` shows the protection and queue settings on `main`; a PR that breaks
parity is blocked from merging while one that passes merges through the queue, both recorded in
LOOP.md with the commands used.

#### HK-11 Local deploy loop — 2026-09-09 · cd5bf50
After a merge nobody rebuilds and restarts the console on the Mac mini. `scripts/deploy-local.sh`
pulls `main` fast-forward, rebuilds the engine and the bundle when the tree changed, and restarts
the service only when no job or study is `running` (the pid in `data/ui/api.pid`, verified
against the listener), writing the new pid; a schedule (launchd, or the app's scheduled tasks)
runs it every few minutes.
**Done when:** with a new commit on `origin/main` the script rebuilds, restarts, and prints the
new pid; with a running job it refuses and says why; with nothing new it exits quickly without
touching the service; a self-test covers those decisions against a stubbed service.

#### HK-13 web/node_modules lives outside iCloud — 2026-09-09 · c97c3ee
The checkout sits in iCloud Drive, which evicts `web/node_modules` under disk pressure: during
HK-09, 2,869 of its 3,697 files were dataless placeholders and `npm run lint` sat in file stats
for ten minutes fetching them one by one, stalling `scripts/check.sh`. Keep the dependencies out
of the synced tree: `scripts/check.sh` (and `npm ci`, through a documented step) should place
them in a non-synced directory such as `~/Library/Caches/tessera/web-node_modules` and leave
`web/node_modules` as a symlink to it, for the main checkout and every worktree.
**Done when:** after `npm ci`, `web/node_modules` is a symlink into a directory outside iCloud,
`ls -lO` finds no dataless file under it, and `scripts/check.sh --resolve` reports where it lives.
Resolved differently: the whole checkout moved to `~/Code/Tessera` (with the private repo beside
it), outside iCloud, after launchd also proved unable to read a script under Documents; nothing
under the checkout is evicted any more, so no symlink is needed.

#### HK-12 Self-hosted runner for the full check — 2026-09-09 · 5a47c36 — Blocked by HK-10
The public runner cannot run the private checks or the layout check against real data. A
self-hosted runner on the Mac mini could run the full `scripts/check.sh` (private checks, the
real console) on PRs. A public repository lets fork PRs run code on the runner, so the job must
skip fork PRs, the repository must require approval for outside collaborators, and neither market
data nor the private checkout may reach CI logs. Decide first whether the trade-off is worth it.
**Done when:** a `full-check` job on the self-hosted runner logs `private checks passed` on a PR
from this repository and is skipped on a fork PR; the runner's setup and security settings are
in `docs/LOOP.md`.
Resolved differently: decided against a runner (docs/LOOP.md, "What CI does not run, and why
there is no self-hosted runner"). A public repository's self-hosted runner on the machine that
holds the data and the private checkout is more exposure than the enforcement is worth while
every PR comes from a checkout that runs the full check first; HK-18 covers the post-merge
case in the deploy loop instead. Revisit when PRs come from elsewhere.

#### HK-14 Loop settings live in `.loop.toml`, not in the scripts — 2026-09-09 · 1d9b250
The loop scripts carry Tessera in their bodies: `scripts/open-ticket-pr.sh` hard-codes
`origin/main` and the parity path `examples/expected/` as the one change that forces a human
review, and the prompts name `scripts/check.sh --no-web` and Tessera's data rules. Another
project cannot adopt the loop without editing every file. Add a flat `.loop.toml` at the repo
root (TOML is only the config format; it says nothing about the project's language) with a
single `[loop]` table: `default_branch`, `backlog` (path of the ticket file), `check` (the full
check command), `check_fast` (the command to run while iterating), `review_paths` (globs whose
change turns off auto-merge), and `trailer_required` (whether commits must carry a
`Co-Authored-By` trailer naming the agent that did the work). A small reader,
`scripts/loop-config.sh <key>`, prints one value with the defaults that reproduce today's
behaviour when the file or key is missing, and `backlog-status.sh`, `open-ticket-pr.sh`, and
`release-notes.sh` read every project-specific value through it.
**Done when:** `scripts/loop-config.sh --self-test` covers a missing file, a missing key, and
each key set; `open-ticket-pr.sh` labels a branch `needs-review` when a configured
`review_paths` glob matches and not otherwise (proved in a fixture repo the self-test builds);
`grep -n 'examples/expected\|origin/main' scripts/open-ticket-pr.sh scripts/backlog-status.sh
scripts/release-notes.sh` finds only comments.

#### HK-15 Instructions and prompts any coding agent can read — 2026-09-09 · 3fa9509 — Blocked by HK-14
`CLAUDE.md` and `.claude/commands/*.md` are read by Claude Code alone; Codex, OpenCode, and the
other harnesses read `AGENTS.md` and have no slash commands. Move the standing instructions to
`AGENTS.md` and leave `CLAUDE.md` as one line that points at it. Move the prompt bodies to
`loop/prompts/next-ticket.md` and `loop/prompts/grill-me.md`, written against the `.loop.toml`
contract only (run the fast check while iterating, the full check before committing, sign with
the trailer the config requires) and free of project names, build tools, and harness tool
names (`AskUserQuestion` becomes "ask the owner, four questions at a time, in whatever way
the harness offers"). The `.claude/commands/*.md` files become two-line wrappers that say to
read and follow the prompt file with the arguments given. Project-specific rules (data, UI
conventions, docs to keep current) stay in `AGENTS.md` under a "Project rules" heading the
prompts refer to by name.
**Done when:** `scripts/check.sh` gains a step that fails if `loop/` mentions `tessera`,
`cargo`, `npm`, `Claude`, or `examples/expected` (case-insensitive; `.loop.toml` is exempt,
being this project's own values, as HK-14 found); `CLAUDE.md` is
a single pointer line; both wrappers under `.claude/commands/` are under five lines; and a
`/next-ticket` run from this checkout still claims, works, and opens a PR for a ticket.

#### HK-16 The loop kit is its own repository — 2026-09-10 · 961a658 — Blocked by HK-15
Once the scripts and prompts are generic they belong in one place every project pulls from.
Create a public repository (working name `loop-kit`; the owner picks the final name) holding
`scripts/backlog-status.sh`, `scripts/open-ticket-pr.sh`, `scripts/release-notes.sh`,
`scripts/loop-config.sh`, `loop/prompts/*.md`, an `AGENTS.md` template with the loop section
and an empty "Project rules" heading, a `.loop.toml` example, the CI workflow skeleton
(one job per configured check command, job names used as the required status checks), the
branch-ruleset JSON from `docs/LOOP.md`, and an `install.sh` that copies those files into a
target checkout and prints what the project still has to supply: `scripts/check.sh` and,
optionally, a deploy script. GitHub is the only hosting assumption (`gh`, rulesets,
auto-merge); say so in its README. Tessera consumes the kit through `scripts/loop-kit-sync.sh`,
which copies the kit's files in and diffs them, so the kit stays the source of truth.
**Done when:** in a fresh `git init` repo containing only a stub `scripts/check.sh`, `install.sh`
followed by `scripts/backlog-status.sh --self-test`, `scripts/release-notes.sh --self-test`,
and `scripts/loop-config.sh --self-test` all pass; and in this checkout
`scripts/loop-kit-sync.sh --check` reports no difference from the kit's tagged release.
Resolved in two steps: the kit is built and proven as the `loop/` directory of this checkout
(laid out as the repository, with `install.sh --self-test` covering the fresh-repository
case and `loop-kit-sync.sh --check` passing against `kit = "loop"`); creating the public
repository, pushing `loop/` to it, and tagging it is the owner's, as HK-19.

#### HK-19 The loop kit is published and this checkout consumes it by tag — 2026-09-10 · 6b41ab8
HK-16 built the kit as `loop/` and proved it; the repository itself needs the owner: pick the
name (working name `loop-kit`), create it under `FueledByChai` as public, push the contents of
`loop/` to it (`git subtree split --prefix=loop` keeps the history, or a plain copy), tag
`v0.1.0`, and apply its own ruleset from `ci/ruleset.json`. Then in this checkout set `kit` to
the repository URL and `kit_ref` to the tag in `.loop.toml`, run `scripts/loop-kit-sync.sh`,
and remove from `loop/` everything the sync now supplies, leaving only `loop/prompts/`. From
then on a change to a loop script is made in the kit, tagged, and pulled in by moving
`kit_ref`.
**Done when:** `scripts/loop-config.sh kit` prints a `https://github.com/` URL and `kit_ref`
a tag; `scripts/loop-kit-sync.sh --check` passes against them from a clean clone; `loop/`
holds only `prompts/`; and `docs/LOOP.md` names the kit repository.

#### HK-17 CI installs Playwright's browser without depending on the apt mirror — 2026-09-10 · 14a6d34
PR #6's web job failed in 24 s with `Failed to install browsers` after an apt index hash
mismatch inside `npx playwright install --with-deps chromium`; the change was docs only, and a
rerun is the only remedy. Cache the browser under `~/.cache/ms-playwright` keyed on the
Playwright version in `web/package-lock.json`, and retry the install once before failing.
**Done when:** the workflow shows a cache step for the browser and a retry around the install,
and two consecutive CI runs on the same lock file show the second restoring the cache.

#### HK-18 Deploy loop runs the private checks before it restarts the service — 2026-09-10 · aedb9a6
Public CI cannot run the private checks (HK-12 decided against a self-hosted runner), so a
merge that breaks the private legacy crate or a private strategy test reaches `main` unseen
until someone runs `scripts/check.sh` here. `scripts/deploy-local.sh` should run the private
checks (`scripts/check.sh --no-web`, or the private step alone when `check.sh` grows a flag for
it) after pulling and before restarting the service, refuse the restart when they fail, and
write the failure to `data/ui/deploy.log` so the owner sees it the next morning.
**Done when:** the deploy script's self-test shows a fixture where the private check fails
leaving the service untouched with `private checks failed` in the log, and one where it passes
and the restart proceeds; `docs/LOOP.md` names it as the post-merge guard.

#### HK-20 backlog-status --next judges done against origin, not a lagging local main — 2026-09-10 · ea1fe80
Right after HK-17 merged, `scripts/backlog-status.sh --next` in the main checkout named HK-17
again: it reads the local `main`, which the deploy loop had not yet pulled, while the claim
branch was already gone. The script should fetch and judge done against `origin/<default
branch>` when that ref exists (falling back to the local branch without a remote), so a
lagging checkout never re-offers a merged ticket. This is a kit script: change it in
`coding-agent-loop`, tag a release, move `kit_ref`, and sync.
**Done when:** the backlog-status self-test shows a ticket landed on `origin/main` but not on
the local `main` reported `done` and skipped by `--next`; `.loop.toml` points at the kit tag
that carries it.

#### HK-21 Proof gate in the kit: code changes must touch a proof — 2026-09-10 · c79ed0d
Nothing enforces the rule that a ticket ships the test proving its done line; a PR that only
touches `src/` merges on green. Add `scripts/proof-gate.sh` to `coding-agent-loop`, configured
by three new `.loop.toml` keys read through `loop-config.sh`: `code_paths` (globs whose change
needs a proof), `proof_paths` (globs that count as proof), and `proof_pattern` (a regex; an
added line matching it in any changed file counts as proof, so a new test function in the same
file qualifies). Empty keys mean the gate is off. It diffs `origin/<default branch>...HEAD`,
fails naming the code files when nothing counts as proof, and passes, printing the reason, when
any commit body in the range has a line `No new test: <reason>`. Serves BT-901.
**Done when:** the kit's proof-gate self-test proves: a code change without proof fails and
names the file; one with a changed proof path passes; one adding a line matching the pattern
passes; one with the body line passes and prints the reason; a change outside `code_paths`
passes; unset keys pass with "gate off". The kit is tagged and `.loop.toml` here moves
`kit_ref` to it.

#### HK-22 Coverage ratchet in the kit: a floor that only rises — 2026-09-10 · c36ecca — Blocked by HK-21
Add `scripts/coverage-ratchet.sh` to `coding-agent-loop` with two `.loop.toml` keys:
`coverage` (a command that prints one percentage, the project's own tool wrapped) and
`coverage_floor` (the floor file, default `coverage-floor.txt`). The script runs the command,
fails below the floor naming both numbers, passes at or above it, says "raise the floor to N"
when above, and `--set` writes the measured number as the new floor. An unset `coverage` key
passes with "ratchet off". Serves BT-902.
**Done when:** the kit's coverage-ratchet self-test, with a stub command, proves: below fails;
equal passes; above passes and names the new floor; `--set` writes it; an unset key reports
off; a command that prints no number fails with a message. The kit is tagged and `kit_ref`
moves to it.

#### HK-23 Tessera measures coverage with cargo-llvm-cov and gates on the floor — 2026-09-10 · 6ce9259 — Blocked by HK-21, HK-22
Wire this checkout to both kit scripts. `.loop.toml`: `code_paths = ["src/"]`, `proof_paths =
["tests/", "examples/expected/", "web/fixtures/", "web/scripts/*-check.mjs"]`, `proof_pattern`
matching `#[test]` or `#[cfg(test)]`, `coverage = "scripts/coverage.sh"`. `scripts/coverage.sh`
runs cargo-llvm-cov over the whole crate and prints the line percentage, failing with the two
install commands (`rustup component add llvm-tools-preview`; `cargo install cargo-llvm-cov`)
when the tool is missing. `scripts/check.sh` runs the proof gate in every mode but
`--web-only` and `--private-only`, and the ratchet in the full check and `--quick` but not
`--no-web`; the CI engine job installs cargo-llvm-cov and fetches enough history for the gate's
diff. The first floor is the number measured on `main`. Serves BT-901 and BT-902.
**Done when:** `scripts/check.sh` prints "coverage N% >= floor N%" and "proof gate: ok" on
`main`; `coverage-floor.txt` is checked in; the CI engine job is green with the tool installed;
a scratch branch that adds a function to `src/` with no test fails the gate locally, recorded
in the commit body.

#### HK-24 Agent review prompt and status in the kit — 2026-09-10 · 78fad60 — Blocked by HK-21
Add `prompts/review-prs.md` and `scripts/review-status.sh` to `coding-agent-loop`. The script
lists open PRs whose head sha has no "Agent review" status (`--pending`), and posts one
(`<sha> pass|fail "<description>"`) through `gh`. The prompt: for each pending PR, read the
ticket the title names (its block in the backlog, its done line), the diff, and the Project
rules in `AGENTS.md`; post a PR review comment with findings; post the status red only for a
missing proof, an unmet done line, a rules breach, or a defect named with file and line, green
otherwise. A PR without a ticket id (a `backlog/` branch) is reviewed against the backlog format
alone. The README documents the owner's override: one `gh` command posting a green status with
a reason. Serves BT-903.
**Done when:** the kit's review-status self-test, with a stub `gh`, proves `--pending` lists
only heads without a status and `pass`/`fail` post the right state and description; the prompt
exists and the kit's generic grep passes; the kit is tagged and `kit_ref` moves to it.

#### HK-25 Tessera PRs wait for the agent review — 2026-09-10 · 1adecff — Blocked by HK-24
Wire the review in: `.claude/commands/review-prs.md` wraps the prompt; `docs/LOOP.md`
describes the scheduled task (desktop app, every 10 minutes, like nightly-studies), the override
command, and the ruleset change; the owner adds "Agent review" to the required status checks
with the `gh` command the ticket carries. Serves BT-903.
**Done when:** a PR from this repository shows an "Agent review" status posted by a scheduled
run; `gh api repos/FueledByChai/tessera/rules/branches/main` lists the context as required;
auto-merge on a green PR waits for it and fires after it; `docs/LOOP.md` has the override
command.

#### HK-26 Coverage ratchet tolerates measurement jitter — 2026-09-10 · c95bbd2
Two runs of `scripts/coverage.sh` on the same HK-23 commit measured 55.5% and then 55.6%:
the instrumented test suite does not cover exactly the same lines every run (timing and
ordering in a few tests). With an exact comparison a run that lands a tenth below the floor
fails a PR that changed nothing. Add a `coverage_slack` key to the kit's ratchet (default 0,
this checkout 0.2): the check fails only when the measurement is below floor minus slack, and
"raise the floor" is suggested only when it exceeds the floor by more than the slack, so the
floor still only moves up. Kit change, then a tag and a sync here.
**Done when:** the kit's coverage-ratchet self-test proves a measurement inside the slack
passes without suggesting a raise, one below floor minus slack fails, and one above floor
plus slack suggests the raise; `.loop.toml` here sets the slack and `kit_ref` the tag.

#### HK-27 One cargo target directory for the main checkout and every worktree — 2026-09-10 · 94d3e5d
Every ticket starts in a fresh worktree with an empty `target/`, so the full check compiles
the 397 dependency crates from cold twice (release, then instrumented for coverage) before
touching Tessera's own 24,000 lines: a docs-only ticket takes six minutes to check, and
Polars gets compiled again though nothing in it changed. Cargo keys artifacts by source
hash, features, and flags, so one shared target directory serves them all. `scripts/check.sh`
(and `scripts/coverage.sh`) should set `CARGO_TARGET_DIR` to the main checkout's `target/`
when run from a worktree (`--resolve` reports it), and the deploy loop and the scratch console
keep using the main checkout's binaries as they do now.
**Done when:** `scripts/check.sh --resolve` from a worktree names the shared target
directory; a docs-only change checked from a fresh worktree right after a full check in the
main checkout finishes the full check in under three minutes, the time recorded in the
commit body; the engine job on CI is unaffected.

#### HK-28 The coverage ratchet skips itself when no code changed — 2026-09-10 · 5556e9c
Coverage cannot move when nothing under `src/` changed, yet the ratchet runs its instrumented
build on every docs-only and scripts-only ticket. `scripts/check.sh` should run the ratchet
only when the diff against `origin/main` touches a `code_paths` entry (the proof gate already
computes this; a `scripts/proof-gate.sh --code-changed` query, or the same test inline, tells
it), printing "coverage ratchet: skipped, no code change" otherwise. On `main` itself, and
in CI's push run, the ratchet still runs.
**Done when:** a docs-only branch's full check prints the skip line and finishes without an
instrumented build; a branch touching `src/` still prints the coverage line; the kit's
proof-gate self-test covers the query if one is added.

#### HK-29 The proof gate sees the working tree, not only commits — 2026-09-10 · 73fe30f
Run locally before the commit, `scripts/proof-gate.sh` reported "no code change" on WB-14's
worktree, whose src/ changes were still uncommitted: it diffs `origin/<default>...HEAD`, so
a full check run before committing (the normal order) never exercises the gate, and only CI
does. The kit script should diff the working tree against the base (`git diff --name-only
origin/<default>` and the same for added lines), so the gate judges what is about to be
committed; the override line is still read from the commits in the range. Kit change, tag,
sync.
**Done when:** the kit's proof-gate self-test adds an uncommitted code change with no proof
and sees the gate fail before any commit, and pass once a proof file is added, still
uncommitted; `kit_ref` here moves to the tag.

#### HK-30 The deploy loop sees the listener under launchd and confirms the restart — 2026-09-10 · 770d7d6
On 2026-09-10 the loop logged "restarted the service: pid 45285" for WB-14 while the console
kept running the September 9 binary: under launchd `lsof` is not on the PATH the plist sets
(it lives in /usr/sbin), so `listener_pid` printed nothing, the old service was never
stopped, the new one died with "Address already in use", and the health probe that followed
was answered by the old process. `scripts/deploy-local.sh` should call `/usr/sbin/lsof` by
absolute path (and `--launchd` should put /usr/sbin on the PATH), and `restart_service`
should confirm the listener's pid equals the pid it just started before reporting a restart,
failing loudly (exit 4, the marker file, the log line) when it does not. The stub in the
self-test should be able to leave the old pid bound so the case is covered.
**Done when:** the deploy self-test has a fixture where the stub keeps the old listener
alive and the run reports "restart failed: the old service still holds the port" instead
of a restart; `--launchd` output contains /usr/sbin; the script has no bare `lsof`.

#### HK-31 Decision records, and grill-me that reads, writes, and respects them — 2026-09-11 · c7dc6ae
Nothing records why a decision was made; Tessera's locked list is bullets without
alternatives, and grill-me guesses or buries a choice in a ticket when a story implies one.
In `coding-agent-loop`: `templates/decision.md` (Context, Decision, Alternatives,
Consequences, "What would show this was wrong", Status: accepted | superseded by NNNN) and
`scripts/decisions.sh` (`new "<title>"` creates the next numbered file from the template,
`index` rewrites `docs/decisions/README.md`, `--check` fails when the index and the files
disagree, `--self-test`). `prompts/grill-me.md` changes: grounding reads the index; a
minimum of three rounds with the last on proofs and edge cases only, and no story drafted
without a named proof; when a story implies a decision no record covers, stop, ask, write
the record in the session, cite it in the ticket; when a story contradicts a record, ask
which wins and write a superseding record if the decision changes. The kit's check asserts
the prompt carries those rules (the phrases "at least three rounds", "docs/decisions",
"superseding"). `.loop.toml` gains `decisions` (the directory, default `docs/decisions`).
Serves BT-904, BT-906.
**Done when:** `scripts/decisions.sh --self-test` proves new, index, --check on a mismatch,
and a superseding record marking its predecessor; the kit's check fails when a required
phrase is removed from the prompt (proved in its self-test by a copy with the phrase
deleted); the PR body records one real grill-me run showing three rounds, a decision record
written mid-session, and its citation in a ticket; the kit is tagged and `kit_ref` here
moves to it.

#### HK-32 grill-project: the first-day interview that leaves rules, decisions, epics, and a check behind — 2026-09-11 · 3892b56 — Blocked by HK-31
A new project has no Project rules, no decisions, and no check, so `/next-ticket` cannot
run. Add `prompts/grill-project.md` and its wrapper to `coding-agent-loop`: rounds over who
and where, the data, runtime and deploy, the UI, and the non-negotiables, each ending in a
decision record or a dated deferral; then it writes the Project rules section of
`AGENTS.md`, the records, the first epics in the product backlog, and `scripts/check.sh`
from a stack table in the prompt (Rust, Python, Node/TypeScript, Java with Maven or Gradle,
Go: test, lint, format, build, coverage command for the ratchet; TODO lines for any other
stack), plus `.loop.toml` filled from the answers. `install.sh` installs the wrapper and the
README says to run it first. Serves BT-905.
**Done when:** the kit's check asserts the prompt names the five areas and the stack table
has the five stacks; `install.sh --self-test` sees the wrapper installed; the PR body
records one real run on a scratch project (a throwaway directory) that ends with
`AGENTS.md` Project rules filled, at least three records, one epic, and a `scripts/check.sh`
that runs and exits 0 on the empty project; the kit is tagged and `kit_ref` here moves to
it.

#### HK-35 The kit sync replaces files atomically, so it cannot break itself mid-run — 2026-09-11 · eb9efef
Syncing v0.9.0 here, `scripts/loop-kit-sync.sh` copied the kit's newer copy of itself over
the file bash was still reading (`cp` rewrites the same inode), bash read a half-replaced
script and stopped with "unexpected EOF", and the templates after it in the list were not
copied until a second run. The kit script should write each file beside its target and
rename it into place (`cp` to `<dst>.tmp`, then `mv -f`), which gives the running shell the
old inode to finish on, and copy its own file last. Kit change, tag, sync.
**Done when:** the kit's loop-kit-sync self-test includes a kit whose loop-kit-sync.sh
differs from the checkout's copy and shows one run copying every file, including a file
listed after the script itself; `kit_ref` here moves to the tag.

#### HK-36 grill-project asks for the project's purpose as its own first question — 2026-09-11 · aa8c1a6
The recorded HK-32 run opened with the shape questions (who, how many, from where, how
often) and only got the one-sentence purpose on the third ask: the owner picked an option
label instead of typing, and in a harness whose question tool offers options, free text
arrives only through its "Other" box. `prompts/grill-project.md` should make round 1 open
with a single free-text question, "what is it, in a sentence: what goes in, what comes out,
what it replaces", with the prompt saying to type the sentence, before the shape questions;
and the restatement at the top of round 2 should quote that sentence back. Kit change,
tag, sync.
**Done when:** `scripts/prompt-check.sh` asserts the phrase "in a sentence" in
`grill-project.md`; a recorded run in the kit PR shows the purpose captured in round 1.

#### HK-34 grill-me sketches the screen a story touches — 2026-09-11 · d6a9278 — Blocked by HK-31
A ticket that changes a page describes its panel and columns in prose, and the layout gets
decided by whoever implements it. In `prompts/grill-me.md`: when a story touches a screen,
the draft carries a text wireframe (a fenced block, at most 80 columns, naming the panels,
tables, and controls and their order) that the owner confirms, and the ticket points at it
by story id; when the owner asks, or the screen is new, the prompt uses the harness's design
canvas for a mockup and the ticket links it. The kit's check asserts the prompt carries the
wireframe rule. Serves BT-906.
**Done when:** the kit's check fails when the wireframe rule is removed from the prompt; the
PR body records one real grill-me run on a screen-touching idea whose draft carries a
wireframe and whose ticket points at it; the kit is tagged and `kit_ref` here moves to it.

#### HK-37 The kit's installer writes CLAUDE.md so Claude Code loads AGENTS.md — 2026-09-11 · 9ca1a6f
`install.sh` writes `AGENTS.md`, which Codex reads on its own, but Claude Code reads
`CLAUDE.md`; a new project run from Claude Code has no standing instructions until someone
adds the one-line file by hand (here it is `@AGENTS.md`). The installer should write
`CLAUDE.md` containing `@AGENTS.md` when absent and keep an existing one; the README's
install section says which harness reads which file. Kit change, tag, sync.
**Done when:** `install.sh --self-test` sees `CLAUDE.md` installed with the `@AGENTS.md`
line and kept on a second install; `kit_ref` here moves to the tag.

#### HK-38 grill-project writes the workflow's toolchain step from the stack table — 2026-09-11 · 31588e4
The kit's workflow is checkout plus `scripts/check.sh`, with a comment to add language
setup before it; the check skeleton passes on the empty repository, so the gap only shows
on the first real ticket, when CI fails for want of uv, Node, a JDK, or Go. The kit should
carry a setup snippet per stack under `templates/ci/` (the `actions/setup-*` or uv step and
its cache), `grill-project` should copy the stack's snippet into `.github/workflows/loop.yml`
before the run line in the same step that writes `scripts/check.sh`, and `prompt-check`
should assert the prompt names the toolchain step. Kit change, tag, sync.
**Done when:** `install.sh --self-test` sees a snippet per stack installed under
`loop/templates/ci/`; `scripts/prompt-check.sh` asserts "toolchain" in `grill-project.md`;
the kit PR shows one workflow produced from a snippet passing `actionlint` or a YAML parse;
`kit_ref` here moves to the tag.

#### HK-39 A sprint list in .loop.toml steers --next — 2026-09-11 · 9bd528f
`scripts/backlog-status.sh --next` takes tickets in file order, so choosing what to work on
next means moving tickets around the file, and nothing records that a sprint was chosen.
Add `sprint = [...]` to `.loop.toml` (kit: `loop-config.sh`), the tickets chosen for now in
order: `--next` takes the first ready one of them before file order and announces the
fallback, `--sprint` lists them with their states and a summary, the table shows each
ticket's position, and grill-me asks which new tickets go into the sprint and where. Tickets
carry no sprint state. Kit change, tag, sync; here the first sprint is set.
**Done when:** the kit's `backlog-status.sh --self-test` proves sprint order beats file
order, an unknown id is reported, an exhausted sprint falls back, and `--sprint` lists the
sprint with a summary; `loop-config.sh --self-test` parses and defaults the key;
`prompt-check.sh` asserts "sprint" in next-ticket and grill-me; `kit_ref` here moves to the
tag and `scripts/backlog-status.sh --next` names the sprint's first ticket.

#### HK-40 Sprint-day views: open tickets, a ticket or story in full, and derived story status — 2026-09-11 · 4e9c865
Choosing a sprint meant reading two files: the table for titles, the ticket file for a
description, and the product backlog's `Status:` lines, which say Proposed forever. Add to
the kit's `backlog-status.sh`: `--open [--section]` (tickets not done and not in the sprint,
grouped by section, with blockers and the story each serves), `--show <id>` (a ticket with
its state and story, or a story with its derived status and tickets), and `--stories` (every
story in the product backlog, `stories` in `.loop.toml`, with a status derived from git: done
when every ticket that says `Serves BT-nnn` landed, open k/n, unticketed); and
`scripts/sprint.sh add|remove|set|clear` to edit the sprint line. grill-me grounds itself with
`--stories` and `--open`. Kit change, tag, sync. Serves BT-904.
**Done when:** the kit's `backlog-status.sh --self-test` proves the three views against a
fixture product backlog before and after its tickets land; `sprint.sh --self-test` proves
add, `--before`, refusals, remove, set, and clear; `kit_ref` here moves to the tag and
`scripts/backlog-status.sh --stories` here derives a status for every story.

#### HK-42 The review pass rebases the open pull requests a merge left behind — 2026-09-11 · de70e0d
Several agents now work tickets at once (claims keep them apart), but the ruleset wants a
pull request up to date with `main` when its checks pass, so the first merge leaves every
other open PR behind and auto-merge waits for a hand-run `scripts/open-ticket-pr.sh <id>
--update`. Add `--update-all` to the kit's `open-ticket-pr.sh` (rebase every open PR into the
default branch whose merge state is BEHIND, report the counts) and make `review-prs.md` run
it first, reviewing a rebased PR when its new head is pending. Kit change, tag, sync.
**Done when:** the kit's `open-ticket-pr.sh --self-test` proves `--update-all` rebases
exactly the PRs behind the default branch (two of three in the fixture) and prints the
summary; `prompt-check.sh` asserts "--update-all" in `review-prs.md`; `kit_ref` here moves to
the tag and one real `--update-all` run here is recorded in the pull request.

#### HK-33 Tessera's locked product decisions become records — 2026-09-11 · 4f6bc26 — Blocked by HK-31
The fifteen bullets under "Locked product decisions" in `docs/PRODUCT_BACKLOG.md` become
`docs/decisions/0001` to `0015`, each quoting its bullet, with Context reconstructed where
the backlog or the docs say why and "Alternatives: not recorded" where nothing does, and
"What would show this was wrong" written now. The bullets are replaced by one line pointing
at the index. `AGENTS.md` Project rules name the directory. Serves BT-904.
**Done when:** `scripts/decisions.sh --check` passes with fifteen records and an index of
fifteen; each record's Context quotes its bullet; the product backlog's section is the
pointer; `scripts/check.sh` runs the decisions check.

#### HK-47 The deploy loop returns from a restart, and a killed loop does not take the console down — 2026-09-12 · 5c5c0ab
On 2026-09-12 the launchd run of `scripts/deploy-local.sh` that deployed a79c4c4 logged
"private checks passed" at 10:02 and never "restarted": `restart_service` calls
`new="$(start_service "$old")"`, and the subshell in `start_service` that launches the console
(`(cd "$ROOT" && nohup ./target/release/tessera-ui > data/ui/api.log 2>&1 & echo $! > ...)`)
stayed alive as the console's parent with the command substitution's pipe on its stdout, so
`$(...)` never returned; the run sat for two hours, launchd started no new run (one instance at
a time), local main stayed at DS-02 while five engine commits merged, and killing the stuck run
took the console with it because launchd tears down the job's process group. Two fixes:
`start_service` launches the console detached from the caller's file descriptors (the subshell
runs with `>/dev/null 2>&1` and `setsid` or `disown`, the pid file written by the parent), and
the `--launchd` plist sets `AbandonProcessGroup` so a stopped loop never stops the console.
`docs/LOOP.md` names both.
**Done when:** `scripts/deploy-local.sh --self-test` gains a case that runs `start_service`
against a fake service (a script that sleeps and serves nothing) inside `$(...)` and fails
unless it returns within five seconds with the pid file written, and asserts the printed
plist contains `AbandonProcessGroup`.

#### HK-44 The deploy loop keys on the build it last deployed, not on the pull delta — 2026-09-12 · 98b0cc6
`scripts/deploy-local.sh` decides "new" by `origin/main` being ahead of the local `main`
and reads what to rebuild from the pulled range. On 2026-09-11 a session in the main
checkout pulled `main` by hand between runs, so four merged tickets (HK-42, HK-33, UI-04,
UI-06, an engine change among them) were logged as "nothing new" and neither the engine nor
the bundle was rebuilt or the service restarted; the console stayed on the previous build
until a deploy by hand. The loop should record the sha it last deployed (`data/ui/deployed`,
engine and bundle separately) and decide by `main` versus that marker, rebuilding what
changed between them whether or not it did the pull itself; `--dry-run` says which marker is
behind. The Project rules add: never pull `main` in the main checkout by hand, and if you
do, run the loop.
**Done when:** `scripts/deploy-local.sh --self-test` adds a case where the fixture's `main`
is advanced by a hand pull before the run and shows the engine rebuilt and the service
restarted anyway, and a case where the marker matches `main` and nothing is rebuilt;
`AGENTS.md` carries the rule.

