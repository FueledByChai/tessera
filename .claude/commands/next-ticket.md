Work the next ticket in BACKLOG.md to done, autonomously, following CLAUDE.md.

Steps:

1. Run `scripts/backlog-status.sh --next`. It names the first `todo` ticket in `BACKLOG.md`
   whose `Blocked by` tickets have a commit on `main` (`scripts/backlog-status.sh` shows every
   ticket's derived state). If it names none, report that and stop. Read the ticket and mark it
   `doing` in the file.
2. Read the ticket's **Done when** line first. Decide what test, fixture, or output proves it,
   and write that test before the implementation.
3. Work in an isolated worktree when the tool is available; otherwise on a branch named after the
   ticket id. Keep the change to that ticket. Anything else you notice becomes a new `todo` ticket
   at the end of the relevant section, one line of context each.
4. Implement. Run `scripts/check.sh --no-web` while iterating and the full `scripts/check.sh`
   before committing. Fix failures yourself. If the check still fails after three distinct
   attempts, set the ticket to `blocked <what fails and what you tried>` and stop.
5. Commit with the ticket id first in the subject, a body that says what changed and how the
   done line is proven, and the co-author trailer. In the same commit clear the ticket's `doing`
   claim in `BACKLOG.md` (the heading keeps no state; git records done). Never push.
6. Finish with a short report: ticket id, what was built, how it was verified, any new tickets
   added, anything the owner should look at.

Rules: do not touch market data or `artifacts/`; do not restart the service while a job is
running; do not refresh parity baselines unless the ticket itself changes engine results, and then
say so in the commit body.
