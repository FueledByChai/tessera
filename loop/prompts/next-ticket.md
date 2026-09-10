Work the next ticket in the backlog to done, autonomously, following the standing instructions
in `AGENTS.md` (the loop section and the Project rules).

Settings come from `.loop.toml`: `scripts/loop-config.sh backlog` names the ticket file,
`check_fast` the check to run while iterating, `check` the full check, `default_branch` the
branch pull requests target, `trailer_required` whether commits sign with an agent trailer.

Steps:

1. Run `scripts/backlog-status.sh --next`. It names the first `todo` ticket whose `Blocked by`
   tickets have a commit on the default branch (`scripts/backlog-status.sh` shows every
   ticket's derived state). If it names none, report that and stop. Read the ticket, mark it
   `doing` in the backlog file, and claim it once the worktree exists:
   `scripts/open-ticket-pr.sh <id> --claim` pushes `ticket/<id>` to origin. If that refuses,
   another checkout holds the ticket: leave it and take the next.
2. Read the ticket's **Done when** line first. Decide what test, fixture, or output proves it,
   and write that test before the implementation.
3. Work in an isolated worktree when the harness offers one; otherwise on a branch named after
   the ticket id. Keep the change to that ticket. Anything else you notice becomes a new `todo`
   ticket at the end of the relevant section, one line of context each.
4. Implement. Run the fast check while iterating and the full check before committing. Fix
   failures yourself. If the full check still fails after three distinct attempts, set the
   ticket to `blocked <what fails and what you tried>` and stop.
5. Commit with the ticket id first in the subject, a body that says what changed and how the
   done line is proven, and, when the config requires it, a `Co-Authored-By: <agent> <email>`
   trailer naming the agent and model that did the work. In the same commit clear the ticket's
   `doing` claim in the backlog file (the heading keeps no state; git records done). Then
   `scripts/open-ticket-pr.sh <id>` pushes the branch and opens the pull request against the
   default branch (`--body-file` for a fuller report than the commit body). Never push the
   default branch.
6. Finish with a short report: ticket id, the pull request URL, what was built, how it was
   verified, any new tickets added, anything the owner should look at. The owner merges the PR;
   the worktree is released after that.

Rules: the Project rules in `AGENTS.md` apply throughout (what never to touch, when not to
restart anything, which baselines not to refresh unless the ticket itself changes results, and
then say so in the commit body).
