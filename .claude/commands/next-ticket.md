Work the next ticket in BACKLOG.md to done, autonomously, following CLAUDE.md.

Steps:

1. Read `BACKLOG.md`. Take the first ticket whose state is `todo` and whose `Blocked by` tickets
   are all `done`. If none qualifies, report that and stop. Mark it `doing` in the file.
2. Read the ticket's **Done when** line first. Decide what test, fixture, or output proves it,
   and write that test before the implementation.
3. Work in an isolated worktree when the tool is available; otherwise on a branch named after the
   ticket id. Keep the change to that ticket. Anything else you notice becomes a new `todo` ticket
   at the end of the relevant section, one line of context each.
4. Implement. Run `scripts/check.sh --no-web` while iterating and the full `scripts/check.sh`
   before committing. Fix failures yourself. If the check still fails after three distinct
   attempts, set the ticket to `blocked <what fails and what you tried>` and stop.
5. Commit with the ticket id first in the subject, a body that says what changed and how the
   done line is proven, and the co-author trailer. Update the ticket to
   `done <YYYY-MM-DD> <short sha>` in the same commit. Never push.
6. Finish with a short report: ticket id, what was built, how it was verified, any new tickets
   added, anything the owner should look at.

Rules: do not touch market data or `artifacts/`; do not restart the service while a job is
running; do not refresh parity baselines unless the ticket itself changes engine results, and then
say so in the commit body.
