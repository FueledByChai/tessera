# 0025 — A poll that began before a local write is dropped, not merged into it

Status: accepted
Date: 2026-09-14

## Context

The console re-reads `/api/dashboard`, `/api/cost-profiles`, and `/api/automations` every
three seconds and replaces each collection with whatever the poll was answered. A write a
page makes — saving a cost profile, creating or toggling an automation, starring a run —
also updates that collection at once, from the service's reply to the write. When the two
meet, the poll wins: a request that was already in flight when the write landed is answered
from the library as it was before the write, and applying that answer takes the record the
user had just created back out of the table for up to three seconds (UI-15). Found while
proving UI-13, whose check counts the rows after a save and saw seven where eight belonged.

Either the stale answer is dropped or it is merged with what the write added. Both keep the
record; they differ in what the collection holds in the meantime, and in how much the page
has to know about the shape of a record.

## Decision

A poll that began before a local write leaves that collection alone: every write into a
polled collection stamps it, a refresh reads the stamps before it asks, and an answer is
applied only to the collections nothing has written to since. A collection the write touched
keeps what the write put there until the next poll, three seconds later, which does hold the
record.

## Alternatives

- Merge the stale answer with the records the write added — keep any locally written record
  the answer omits, until a later answer includes it. Keeps every other field of the
  collection fresh one cycle earlier, but the page has to remember which records it wrote,
  match them by id, and decide when to forget them, and a record the service has since
  deleted would come back on every poll.
- Re-read after every write: a write would not be lost, but every save would cost another
  round trip, and a poll could still land between the write and the re-read.
- Sequence the polls, skipping a tick while a write is in flight: hides the race rather than
  answering it, and a write from somewhere other than this page still loses.

## Consequences

The console keeps one small write stamp per collection it both polls and writes, and any new
collection of that shape has to stamp it too or its writes stay droppable. Skipping an answer
delays every other change in that collection — a cost another browser saved, an automation
started elsewhere — by up to three seconds after a local write, which is inside the poll's
own resolution anyway. A poll is never dropped whole: the three collections are stamped and
applied on their own, so a write into one does not hold back the other two.

## What would show this was wrong

A record a write added disappearing from a table, or a change made elsewhere taking more than
one poll cycle to appear after a local write, in a collection that stamps its writes.
