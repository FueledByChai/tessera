# 0002 — Alerts surface in the console and the research log, never outbound

Status: accepted
Date: 2026-09-11

## Context

The IC-decay alert (grill-me, 2026-09-11) is the first thing in Tessera that wants to
tell the owner something without being asked. Nothing on record said whether the project
sends notifications, and every channel (email, push, chat) means credentials on the Mac
mini and a first outbound dependency.

## Decision

Alerts surface in two places and nowhere else: a panel on the console's dashboard with the
numbers behind the alert, and a dated line appended to the private research log so the
morning read sees it. Tessera sends nothing outbound.

## Alternatives

- Email or push as well: immediate, but credentials on the machine, a channel to choose,
  and the first outbound anything in a project whose data must stay local.
- Console panel only: no record for the morning read, and an alert that is only seen when
  the console happens to be open.

## Consequences

Every future alert reuses the dashboard panel and the research-log line rather than adding
a channel. The console is the inbox; the research log is the record. An alert that must
reach the owner away from the machine has no path today.

## What would show this was wrong

Missing an alert that mattered because nobody opened the console for days, or a second
alert kind that clearly needs to reach a phone, would reopen this.
