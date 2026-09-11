# 0001 — Scheduled research work runs inside the service's scheduler

Status: accepted
Date: 2026-09-11

## Context

Tessera has three places where work already runs on a schedule: the service's own
`automation_schedules` table (seeded kinds with a local time, weekdays, and last-run status,
shown in the console), agent commands run from the desktop app's scheduled tasks
(`/nightly-studies`, `/review-prs`), and launchd (`scripts/deploy-local.sh`). The first
grill-me session on IC-decay alerts (2026-09-11) needed a home for a nightly rescoring job
and found no record saying which of the three research work belongs in.

## Decision

Scheduled research work that reads the catalog and the market data and writes results back
runs inside the service, as a kind in `automation_schedules`, in process, with its last run
and status visible in the console. Agent commands are for work that needs judgement or
writes prose (the research log, reviews); launchd is for the deploy loop alone.

## Alternatives

- Extend `/nightly-studies` and schedule it from the desktop app: reuses the research-log
  habit, but depends on an agent session being up and on a scheduler outside the service,
  and a job that only computes numbers does not need an agent.
- A launchd job calling the CLI: independent of the service, but a second scheduler to keep
  alive and a second place to look when something did not run.

## Consequences

The service owns a job runner and its status table; a new scheduled computation is a new
kind there, with tests against an in-memory catalog like the other seeded kinds. Anything
that needs prose or judgement stays an agent command. The console is where "did it run"
is answered.

## What would show this was wrong

A scheduled job that needs more than the service can give it (a model call, an agent's
judgement, another machine's data), or the service's scheduler proving unreliable for
jobs longer than a few minutes, would reopen this.
