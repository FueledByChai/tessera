#!/usr/bin/env bash
# The loop's state as one screen (LK-01). One renderer turns the loop's scripts' output into
# frames at a given width; this file holds the dashboard. It is split from the terminal:
# --render prints one frame and exits, --width fixes the width, and it takes no keys and writes
# nothing, so the interactive mode (LK-03) is a key handler over these frames rather than a
# second implementation.
#
#   scripts/loop-tui.sh                  the dashboard: one frame on stdout, then exit
#   scripts/loop-tui.sh --render         the same, spelled out
#   scripts/loop-tui.sh --width <n>      fix the width (default: the terminal's, else 78)
#   scripts/loop-tui.sh --ref <ref>      the ref done is judged against (default:
#                                        origin/<default_branch> when it exists, else the
#                                        branch itself). Nothing is fetched here: the frame shows
#                                        what the checkout already knows, and LK-03's `r` is
#                                        what fetches.
#   scripts/loop-tui.sh --self-test      a fixture repository proves the frame, by named line and
#                                        by golden frames at 60, 78, and 200 columns
#
# The data comes from the loop's own scripts, never from a second reading of the backlog:
# scripts/backlog-status.sh --sprint gives the rows and the counts, --open what sits outside the
# sprint, --stories the story line, and --next the ticket to work when the sprint holds nothing
# ready. Each call is a subprocess, and the one that resolves claims reaches origin, so the frame
# makes three calls, and a fourth only when the sprint has no ready ticket to name - a frame per
# ticket would cost about a second each (measured: scripts/backlog-status.sh --show is ~1.4s,
# mostly the fetch).
#
# The table columns are cut by position because that is how scripts/backlog-status.sh prints
# them (%-6s %-8s %-10s %-8s %-6s %-6s %-22s %s). The state is the one field that can hold a
# space, so a `blocked <reason>` claim makes it wider than its 8 and shifts the rest of the row.
# The boundary is found by testing each date-or-dash against the fixed columns that follow it,
# because the reason itself can hold a date or a lone dash.
#
# Settings, from .loop.toml through scripts/loop-config.sh: default_branch, backlog, stories.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
STATUS="$SCRIPT_ROOT/scripts/backlog-status.sh"
MAX_ROWS=10

WIDTH=""
REF=""
MODE=dashboard
while [ $# -gt 0 ]; do
  case "$1" in
    --render) MODE=dashboard ;;
    --width) WIDTH="${2:-}"; shift ;;
    --ref) REF="${2:-}"; shift ;;
    --self-test) MODE=selftest ;;
    *) echo "usage: scripts/loop-tui.sh [--render] [--width <n>] [--ref <ref>] | --self-test" >&2; exit 2 ;;
  esac
  shift
done

# The ref the frame judges done against: what the checkout already has, never a fetch. The
# remote-tracking branch is preferred when it exists, which is what scripts/backlog-status.sh
# would pick after its fetch, so the two agree once something has fetched.
default_ref() {
  local branch; branch="$("$CONFIG" default_branch)"
  if git rev-parse -q --verify "refs/remotes/origin/$branch" >/dev/null 2>&1; then
    echo "origin/$branch"
  else
    echo "$branch"
  fi
}

# The width to draw at: --width, else the terminal's, else 78 so a pipe has a stable default.
frame_width() {
  if [ -n "$WIDTH" ]; then echo "$WIDTH"; return; fi
  local cols=""
  if [ -t 1 ] && command -v tput >/dev/null 2>&1; then cols="$(tput cols 2>/dev/null || true)"; fi
  case "$cols" in ''|*[!0-9]*) cols=78 ;; esac
  [ "$cols" -ge 40 ] || cols=40
  echo "$cols"
}

# A failed scripts/backlog-status.sh is not an empty sprint. Given a ref that does not exist it
# reports every ticket as unfinished, so swallowing the failure would print counts and a claim
# command for tickets that already landed - a frame that reads as project state while being
# false. Say what failed, print no figures, and exit non-zero.
error_frame() {
  local width="$1" ref="$2" message="$3" line
  local rule; rule="$(printf '%*s' "$width" '' | tr ' ' '-')"
  echo "TICKET LOOP  backlog-status.sh failed"
  echo "$rule"
  echo " The ref it was given: $ref"
  while IFS= read -r line; do
    [ -n "$line" ] && echo "   $line"
  done <<EOF
$message
EOF
  echo "$rule"
  echo " Fix the ref, or drop --ref to judge done against the default branch."
}

# Gathers the three scripts' output and draws the frame. The data is handed to perl as
# environment, which keeps the renderer free of temp files.
render() {
  local ref="$1" width="$2"
  local sprint open_line stories_line project branch sha backlog
  # scripts/backlog-status.sh joins the configured ticket file to its own root, which is the
  # kit's wherever the frame is run; name the file outright so the frame describes the project
  # it is run in. The product backlog needs no such help: that script resolves it against the
  # working tree's top level, which is the project.
  backlog="$ROOT/$("$CONFIG" backlog)"
  # Nothing to draw from: say which file is missing rather than print an empty table (LS-01).
  if [ ! -f "$backlog" ]; then
    local rule; rule="$(printf '%*s' "$width" '' | tr ' ' '-')"
    echo "TICKET LOOP  no ticket file"
    echo "$rule"
    echo " The ticket file .loop.toml names is not there:"
    echo "   $backlog"
    echo " Write one (the grill-project prompt does) or point backlog at it."
    echo "$rule"
    return 0
  fi
  # The rows and the counts come from --sprint, so a failure there is not "no tickets in the
  # sprint": keep the message and draw an error frame rather than a false one.
  local sprint_out ready_count next_id
  if sprint_out="$("$STATUS" --sprint --ref "$ref" --backlog "$backlog" 2>&1)"; then
    sprint="$sprint_out"
  else
    error_frame "$width" "$ref" "$sprint_out"
    return 1
  fi
  # --local on the two calls that never look at a ticket's state: neither the count of what is
  # outside the sprint nor a story's derived status depends on a claim, and --local skips the
  # git ls-remote that the claim lookup would run against origin.
  open_line="$("$STATUS" --open --ref "$ref" --local --backlog "$backlog" 2>/dev/null | tail -1 || true)"
  stories_line="$("$STATUS" --stories --ref "$ref" --local --backlog "$backlog" 2>/dev/null | tail -1 || true)"
  # NEXT follows --next, which falls back to file order when nothing in the sprint is ready: a
  # ticket outside the sprint is still the ticket to work, and the dashboard must not call that
  # "nothing ready". Ask only in that case - the call resolves claims against origin, and a
  # sprint that holds a ready ticket already answers.
  next_id=""
  ready_count="$(printf '%s\n' "$sprint" | sed -n 's/.*, \([0-9][0-9]*\) ready,.*/\1/p' | tail -1)"
  if [ -z "$ready_count" ] || [ "$ready_count" = 0 ]; then
    next_id="$("$STATUS" --next --ref "$ref" --backlog "$backlog" 2>/dev/null | tail -1 || true)"
  fi
  project="$(basename "$(git rev-parse --show-toplevel 2>/dev/null || pwd)")"
  branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '-')"
  sha="$(git rev-parse --short HEAD 2>/dev/null || echo '-')"
  TUI_WIDTH="$width" TUI_PROJECT="$project" TUI_BRANCH="$branch" TUI_SHA="$sha" \
  TUI_SPRINT="$sprint" TUI_OPEN="$open_line" TUI_STORIES="$stories_line" TUI_MAX="$MAX_ROWS" \
  TUI_NEXT="$next_id" \
    perl -e '
      use strict; use warnings;
      use Encode qw(decode);
      binmode STDOUT, ":encoding(UTF-8)";
      my $d = sub { my $v = shift; return defined $v ? decode("UTF-8", $v) : "" };
      my $width = $d->($ENV{TUI_WIDTH}) + 0;
      my $project = $d->($ENV{TUI_PROJECT});
      my $branch = $d->($ENV{TUI_BRANCH});
      my $sha = $d->($ENV{TUI_SHA});
      my $max = $d->($ENV{TUI_MAX}) + 0;
      # What scripts/backlog-status.sh --next names when nothing in the sprint is ready; empty
      # whenever the sprint answered for itself.
      my $next_id = $d->($ENV{TUI_NEXT});
      my $ELL = "\x{2026}";

      # Cuts a string to a width, marking the cut with an ellipsis rather than splitting a
      # character or a figure in half.
      my $cut = sub {
        my ($t, $w) = @_;
        $t = "" unless defined $t;
        return $t if length($t) <= $w;
        return substr($t, 0, $w) if $w <= 1;
        return substr($t, 0, $w - 1) . $ELL;
      };
      # Pads to a width (never truncates; the caller cuts first).
      my $pad = sub { my ($t, $w) = @_; $t = "" unless defined $t; return $t . (" " x ($w - length($t))) if length($t) < $w; return $t };
      # The same, right-aligned, for the sprint position.
      my $rpad = sub { my ($t, $w) = @_; $t = "" unless defined $t; return (" " x ($w - length($t))) . $t if length($t) < $w; return $t };

      # ---- the sprint rows, parsed out of the table scripts/backlog-status.sh printed ----
      my @rows;
      my $text = $d->($ENV{TUI_SPRINT});
      my @lines = split /\n/, $text;
      my $has_table = (@lines && $lines[0] =~ /^id\s+state\s/);
      if ($has_table) {
        shift @lines;
        for my $line (@lines) {
          next if $line =~ /^\s*$/ || $line =~ /^sprint: /;
          my ($id) = $line =~ /^(\S+)/;
          next unless defined $id;
          # The id column is 6 wide; a longer id pushes the state right by the difference.
          my $pos = (length($id) > 6 ? length($id) : 6) + 1;
          my $rest = substr($line, $pos);
          # The state runs up to the date column. Finding that boundary by the first date-or-dash
          # is not enough: a `blocked <reason>` claim can hold a date of its own ("blocked until
          # 2026-10-01 response") or a lone dash ("blocked owner - waiting"), and splitting there
          # shifts every column after it. So each candidate boundary is tested against the fixed
          # columns that follow the date - the date, the short sha, `ready`, the sprint position -
          # and the first whose tail holds is the real boundary.
          my @starts;
          while ($rest =~ /(?=(?:\d{4}-\d{2}-\d{2}|-) )/g) { push @starts, $-[0] }
          my ($state, $after) = ("", "");
          for my $start (@starts) {
            my $cand = substr($rest, 0, $start);
            my $tail = substr($rest, $start);
            next if length($tail) < 33;
            my ($d10, $h8, $r6, $p6) = (substr($tail, 0, 10), substr($tail, 11, 8),
                                        substr($tail, 20, 6), substr($tail, 27, 6));
            s/\s+$// for ($d10, $h8, $r6, $p6);
            next unless $d10 =~ /^(?:\d{4}-\d{2}-\d{2}|-)$/;
            next unless $h8 =~ /^(?:[0-9a-f]{6,40}|-)$/;
            next unless $r6 =~ /^(?:yes)?$/;
            next unless $p6 =~ /^[0-9]*$/;
            $state = $cand; $after = $tail; last;
          }
          if ($after eq "") {
            # Nothing validated: fall back to the first candidate, or to the first word.
            my $start = @starts ? $starts[0] : length($rest);
            $state = substr($rest, 0, $start);
            $after = substr($rest, $start);
            $state =~ s/\s.*$// if !@starts;
          }
          $state =~ s/\s+$//;
          $after =~ s/^ +//;
          my $ready = substr($after, 20, 6); $ready =~ s/\s+$//;
          my $spos = substr($after, 27, 6); $spos =~ s/\s+$//;
          my $blocked = substr($after, 34, 22); $blocked =~ s/\s+$//;
          my $title = substr($after, 57); $title =~ s/^\s+//;
          $title = "" unless defined $title;
          # The state column holds one word. A `blocked <reason>` claim and a `done (was
          # doing)` both carry prose that belongs in the show view, not in a table cell.
          $state =~ s/^([A-Za-z]+).*/$1/;
          push @rows, { id => $id, state => $state, ready => $ready, blocked => $blocked,
                        title => $title, pos => $spos };
        }
      }
      my $sprint_empty = !$has_table;

      # ---- the counts, from the scripts'"'"' own summary lines ----
      my ($n_total, $n_done, $n_ready, $n_claimed, $n_blocked) = (0, 0, 0, 0, 0);
      if ($d->($ENV{TUI_SPRINT}) =~ /^sprint: (\d+) ticket\(s\), (\d+) done, (\d+) ready, (\d+) claimed or doing, (\d+) blocked or waiting/m) {
        ($n_total, $n_done, $n_ready, $n_claimed, $n_blocked) = ($1, $2, $3, $4, $5);
      }
      my $outside = "0";
      if ($d->($ENV{TUI_OPEN}) =~ /^open: (\d+) ticket/) { $outside = $1 }
      my $stories = $d->($ENV{TUI_STORIES});
      $stories = "none configured" unless $stories =~ /^stories: /;

      # ---- the columns: each is a fixed width, and the widest ones go first when the width
      #      cannot hold the title. A column width includes the single space after it. ----
      my @keep = qw(idx id state ready blocked title);
      my %w = (idx => 4, id => 7, state => 9, ready => 6, blocked => 17);
      my $lead = 1;
      my $fixed = $lead + $w{idx} + $w{id} + $w{state} + $w{ready} + $w{blocked};
      my $MIN_TITLE = 20;
      while ($width - $fixed < $MIN_TITLE) {
        my $dropped = 0;
        for my $c (qw(blocked ready state)) {
          next unless grep { $_ eq $c } @keep;
          @keep = grep { $_ ne $c } @keep;
          $fixed -= $w{$c};
          $dropped = 1;
          last;
        }
        last unless $dropped;
      }
      my $title_w = $width - $fixed;
      $title_w = 8 if $title_w < 8;
      my $has = sub { my $c = shift; return scalar grep { $_ eq $c } @keep };
      my $cell = sub { my ($t, $c) = @_; return $pad->($t, $w{$c} - 1) . " " };

      # The header names the project, the branch and commit, and the sprint counts. It gives
      # up the branch and then shortens the project name rather than cutting a figure in half.
      my $counts = "$n_total in sprint: $n_done done $n_ready ready $n_claimed claimed";
      my $head_full = "TICKET LOOP  $project  $branch\@$sha   $counts";
      my $head_short = "TICKET LOOP  $project   $counts";
      my $head_bare = "TICKET LOOP   $counts";
      my $header;
      if (length($head_full) <= $width) { $header = $head_full }
      elsif (length($head_short) <= $width) { $header = $head_short }
      elsif (length($head_bare) <= $width) { $header = $head_bare }
      else {
        my $room = $width - length("TICKET LOOP     $counts");
        $header = "TICKET LOOP  " . $cut->($project, $room > 3 ? $room : 3) . "   $counts";
      }

      my @out;
      push @out, $header;
      push @out, "-" x $width;
      push @out, " SPRINT";
      if ($sprint_empty || !@rows) {
        push @out, "   (no sprint set - put sprint = [\"AB-01\", ...] in .loop.toml)";
      } else {
        my $head = " ";
        $head .= $rpad->("#", $w{idx} - 1) . " " if $has->("idx");
        $head .= $cell->("id", "id") if $has->("id");
        $head .= $cell->("state", "state") if $has->("state");
        $head .= $cell->("ready", "ready") if $has->("ready");
        $head .= $cell->("blocked", "blocked") if $has->("blocked");
        $head .= "title";
        push @out, $head;
        my $shown = 0;
        for my $r (@rows) {
          if ($shown >= $max) { last }
          my $line = " ";
          $line .= $rpad->($r->{pos} ne "" ? $r->{pos} : "-", $w{idx} - 1) . " " if $has->("idx");
          $line .= $cell->($r->{id}, "id") if $has->("id");
          $line .= $cell->($cut->($r->{state}, $w{state} - 1), "state") if $has->("state");
          $line .= $cell->($r->{ready} ne "" ? $r->{ready} : "-", "ready") if $has->("ready");
          $line .= $cell->($cut->($r->{blocked}, $w{blocked} - 1), "blocked") if $has->("blocked");
          $line .= $cut->($r->{title}, $title_w);
          push @out, $line;
          $shown++;
        }
        push @out, "   " . $ELL . "  (" . (scalar(@rows) - $shown) . " more)" if scalar(@rows) > $shown;
      }
      push @out, "-" x $width;

      # ---- NEXT: what scripts/backlog-status.sh --next names. That script takes the first ready
      #      ticket in sprint order and falls back to file order, so when the sprint holds nothing
      #      ready it still names a ticket; the frame has to say the same thing it would. ----
      my ($next) = grep { $_->{ready} eq "yes" } @rows;
      my $target = $next_id ne "" ? $next_id : ($next ? $next->{id} : "");
      if ($target ne "") {
        my $row = ($next && $next->{id} eq $target) ? $next : undef;
        my $title = $row ? $row->{title} : "(outside the sprint)";
        push @out, " NEXT  $target  " . $cut->($title, $width - 8 - length($target));
        # The claim command is the headline output of the frame, so it is never cut: it loses its
        # indent first, and wraps at the script name if even that will not fit.
        my $cmd = "scripts/open-ticket-pr.sh $target --claim";
        if (length("       -> $cmd") <= $width) { push @out, "       -> $cmd" }
        elsif (length($cmd) <= $width) { push @out, $cmd }
        else {
          push @out, " -> scripts/open-ticket-pr.sh";
          push @out, "    $target --claim";
        }
      } else {
        push @out, " NEXT  nothing ready - every ticket is done, claimed, or waiting on a blocker";
      }
      push @out, "-" x $width;
      # The summary lines give up their tail, clause by clause, rather than being cut across a
      # figure: at a narrow width the count of what sits outside the sprint goes first.
      my $left = " LEFT  $n_ready ready  $n_claimed claimed  $n_blocked blocked   outside the sprint: $outside open tickets";
      $left = " LEFT  $n_ready ready  $n_claimed claimed  $n_blocked blocked   outside: $outside open" if length($left) > $width;
      $left = " LEFT  $n_ready ready  $n_claimed claimed  $n_blocked blocked" if length($left) > $width;
      $left = " LEFT  $n_ready ready  $n_claimed claimed" if length($left) > $width;
      push @out, $left;
      my $story_text = $stories =~ /^stories: (.*)$/ ? $1 : $stories;
      $story_text =~ s/, / - /g;
      push @out, " STORIES  $story_text";
      push @out, "-" x $width;
      push @out, " ? help   <sp> show   a add   x remove   r refresh   q quit";

      for my $line (@out) { print $cut->($line, $width), "\n" }
    '
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/loop-tui.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR"
  local me="$SCRIPT_ROOT/scripts/loop-tui.sh"
  # A fixture project: a bare origin, a checkout with a done, a claimed, a blocked, and a ready
  # ticket, a sprint over them, and a product backlog with a ticketed and an unticketed story.
  (
    cd "$dir"
    git init -q --bare origin.git
    git clone -q origin.git work 2>/dev/null
    cd work
    git config user.email "self-test@example.com"; git config user.name "self-test"
    # Fixed commit dates make the history, and so the short sha the header carries,
    # reproducible: the golden frames are compared byte for byte and cannot hold a moving sha.
    export GIT_AUTHOR_DATE="2026-01-02T03:04:05+00:00" GIT_COMMITTER_DATE="2026-01-02T03:04:05+00:00"
    git checkout -q -b main
    cat > BACKLOG.md <<'EOF'
# Fixture queue

## Alpha

### AA-01 A ticket that landed
Body.
**Done when:** it lands.

### AA-02 A ticket someone holds
Body.
**Done when:** it lands.

### AA-03 A ticket waiting on someone else
Body.
**Done when:** the blocker lands.

### AA-04 A ticket ready to start — Blocked by AA-01
Body. Serves BB-1.
**Done when:** it lands.

### AA-05 A ticket behind an unlanded blocker — Blocked by AA-03
Body.
**Done when:** the blocker lands.
EOF
    cat > PRODUCT.md <<'EOF'
# Fixture product backlog

## Epic A: The first epic

### BB-1 — A story a ticket serves

**Status:** Proposed

### BB-2 — A story nothing serves yet

**Status:** Proposed
EOF
    printf '[loop]\ndefault_branch = "main"\nsprint = ["AA-01", "AA-02", "AA-03", "AA-04", "AA-05"]\nstories = "PRODUCT.md"\n' > .loop.toml
    git add -A
    git commit -q -m "Scaffold"
    git commit -q --allow-empty -m "AA-01: the first ticket landed"
    git push -q -u origin main 2>/dev/null
    # AA-02 is claimed: a ticket/<id> branch on origin.
    git push -q origin "HEAD:refs/heads/ticket/AA-02" 2>/dev/null
    # AA-03 carries a `blocked <reason>` claim, which makes its state wider than its column, and
    # the reason holds a date of its own - the case a parse that splits at the first date gets
    # wrong, shifting every column after it.
    perl -i -pe 's/^### AA-03 (.*)$/### AA-03 $1 — `blocked until 2026-10-01 response`/' BACKLOG.md
    git add -A; git commit -q -m "Backlog: AA-03 is blocked"
  )
  local work="$dir/work"

  # LOOP_TUI_GOLDEN=1 prints the frames the golden heredocs below are cut from, so they can be
  # regenerated after a deliberate layout change instead of guessed at.
  if [ -n "${LOOP_TUI_GOLDEN:-}" ]; then
    local gw
    for gw in 60 78 200; do
      printf '===== %s =====\n' "$gw"
      (cd "$work" && LOOP_ROOT="$work" "$me" --width "$gw")
    done
    exit 0
  fi

  # 1. The dashboard at 78 columns, asserted by named line.
  local out
  out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width 78 2>&1)"
  echo "$out" | grep -q '^TICKET LOOP  work  main@.*   5 in sprint: 1 done 1 ready 1 claimed$' \
    || { echo "self-test: the header is off:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^   1 AA-01  done' || { echo "self-test: the done ticket's row is off:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'AA-02  claimed' || { echo "self-test: the claimed ticket should show as claimed:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'AA-03  blocked' || { echo "self-test: the blocked ticket should show as blocked:"; echo "$out"; exit 1; }
  # AA-03's claim is wider than its column and holds a date of its own, so these two rows prove
  # the parse found the real column boundary: AA-03 keeps its own columns, and AA-05 - the row
  # after it - keeps its blocker and its own state.
  echo "$out" | grep -qE '^   3 AA-03  blocked  - +A ticket waiting on someone else$' \
    || { echo "self-test: a date inside the blocked reason shifted AA-03's own row:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'AA-05  todo     -     AA-03!' || { echo "self-test: the row after a long state should stay aligned:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^ NEXT  AA-04' || { echo "self-test: NEXT should be the first ready ticket:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^       -> scripts/open-ticket-pr.sh AA-04 --claim$' \
    || { echo "self-test: the claim command is off:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^ LEFT  1 ready  1 claimed  2 blocked   outside the sprint: 0 open tickets$' \
    || { echo "self-test: the LEFT counts are off:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^ STORIES  2 - 0 done - 1 open - 1 unticketed$' \
    || { echo "self-test: the stories line is off:"; echo "$out"; exit 1; }

  # 2. Every line fits the width; a narrower frame drops the blocked column. The width is
  #    counted in characters, as the renderer draws them, not in the bytes the ellipsis takes.
  local w line n
  for w in 40 60 78 200; do
    out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width "$w" 2>&1)"
    while IFS= read -r line; do
      n="$(printf '%s' "$line" | perl -CS -ne 'chomp; print length($_)')"
      [ "$n" -le "$w" ] || { echo "self-test: a line is $n wide at width $w:"; echo "$line"; exit 1; }
    done <<EOF
$out
EOF
  done
  out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width 60 2>&1)"
  echo "$out" | grep -q 'ready blocked' && { echo "self-test: 60 columns should drop the blocked column:"; echo "$out"; exit 1; }
  out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width 78 2>&1)"
  echo "$out" | grep -q 'ready blocked' || { echo "self-test: 78 columns should keep the blocked column:"; echo "$out"; exit 1; }
  # The claim command is the headline output of the frame, so at the 40-column floor it gives up
  # its indent rather than being cut into an unrunnable "scripts/open-ticket-pr.sh AA-…".
  out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width 40 2>&1)"
  echo "$out" | grep -q '^scripts/open-ticket-pr.sh AA-04 --claim$' \
    || { echo "self-test: 40 columns cut the claim command:"; echo "$out"; exit 1; }
  # 3. At 200 columns the titles stop being cut.
  out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width 200 2>&1)"
  echo "$out" | grep -q 'A ticket behind an unlanded blocker$' \
    || { echo "self-test: 200 columns should print a title in full:"; echo "$out"; exit 1; }

  # 4. The frames are frozen. A layout change has to be deliberate: run the self-test with
  #    LOOP_TUI_GOLDEN=1 to print these frames again and paste them back, rather than letting
  #    the screen drift unnoticed.
  local golden60 golden78 golden200
  golden60="$(cat <<'G60'
TICKET LOOP  work   5 in sprint: 1 done 1 ready 1 claimed
------------------------------------------------------------
 SPRINT
   # id     state    ready title
   1 AA-01  done     -     A ticket that landed
   2 AA-02  claimed  -     A ticket someone holds
   3 AA-03  blocked  -     A ticket waiting on someone else
   4 AA-04  todo     yes   A ticket ready to start
   5 AA-05  todo     -     A ticket behind an unlanded bloc…
------------------------------------------------------------
 NEXT  AA-04  A ticket ready to start
       -> scripts/open-ticket-pr.sh AA-04 --claim
------------------------------------------------------------
 LEFT  1 ready  1 claimed  2 blocked   outside: 0 open
 STORIES  2 - 0 done - 1 open - 1 unticketed
------------------------------------------------------------
 ? help   <sp> show   a add   x remove   r refresh   q quit
G60
)"
  golden78="$(cat <<'G78'
TICKET LOOP  work  main@ff5d3b3   5 in sprint: 1 done 1 ready 1 claimed
------------------------------------------------------------------------------
 SPRINT
   # id     state    ready blocked          title
   1 AA-01  done     -                      A ticket that landed
   2 AA-02  claimed  -                      A ticket someone holds
   3 AA-03  blocked  -                      A ticket waiting on someone else
   4 AA-04  todo     yes   AA-01            A ticket ready to start
   5 AA-05  todo     -     AA-03!           A ticket behind an unlanded block…
------------------------------------------------------------------------------
 NEXT  AA-04  A ticket ready to start
       -> scripts/open-ticket-pr.sh AA-04 --claim
------------------------------------------------------------------------------
 LEFT  1 ready  1 claimed  2 blocked   outside the sprint: 0 open tickets
 STORIES  2 - 0 done - 1 open - 1 unticketed
------------------------------------------------------------------------------
 ? help   <sp> show   a add   x remove   r refresh   q quit
G78
)"
  golden200="$(cat <<'G200'
TICKET LOOP  work  main@ff5d3b3   5 in sprint: 1 done 1 ready 1 claimed
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 SPRINT
   # id     state    ready blocked          title
   1 AA-01  done     -                      A ticket that landed
   2 AA-02  claimed  -                      A ticket someone holds
   3 AA-03  blocked  -                      A ticket waiting on someone else
   4 AA-04  todo     yes   AA-01            A ticket ready to start
   5 AA-05  todo     -     AA-03!           A ticket behind an unlanded blocker
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 NEXT  AA-04  A ticket ready to start
       -> scripts/open-ticket-pr.sh AA-04 --claim
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 LEFT  1 ready  1 claimed  2 blocked   outside the sprint: 0 open tickets
 STORIES  2 - 0 done - 1 open - 1 unticketed
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 ? help   <sp> show   a add   x remove   r refresh   q quit
G200
)"
  local gw got want
  for gw in 60 78 200; do
    got="$(cd "$work" && LOOP_ROOT="$work" "$me" --width "$gw" 2>&1)"
    case "$gw" in
      60) want="$golden60" ;;
      78) want="$golden78" ;;
      200) want="$golden200" ;;
    esac
    if [ "$got" != "$want" ]; then
      echo "self-test: the frame at $gw columns no longer matches its golden:"
      printf '%s\n' "$want" > "$dir/want.txt"
      printf '%s\n' "$got" > "$dir/got.txt"
      diff "$dir/want.txt" "$dir/got.txt" || true
      exit 1
    fi
  done

  # 5. When nothing in the sprint is ready, NEXT follows --next, which falls back to file order:
  #    the ticket to work sits outside the sprint, and the frame names it and its claim command
  #    rather than saying "nothing ready" while the loop would hand you a ticket.
  printf '[loop]\ndefault_branch = "main"\nsprint = ["AA-01"]\nstories = "PRODUCT.md"\n' > "$work/done-only.toml"
  out="$(cd "$work" && LOOP_CONFIG="$work/done-only.toml" LOOP_ROOT="$work" "$me" --width 78 2>&1)"
  echo "$out" | grep -q '^ NEXT  AA-04  (outside the sprint)$' \
    || { echo "self-test: NEXT should fall back to the ticket outside the sprint:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^       -> scripts/open-ticket-pr.sh AA-04 --claim$' \
    || { echo "self-test: the fallback claim command is off:"; echo "$out"; exit 1; }

  # 6. A failed scripts/backlog-status.sh is an error, not an empty sprint. A ref that does not
  #    exist makes it report every ticket as unfinished; rendered, that would print counts and a
  #    claim command for a ticket that already landed.
  if out="$(cd "$work" && LOOP_ROOT="$work" "$me" --width 78 --ref nosuchref 2>&1)"; then
    echo "self-test: a --ref that does not exist must exit non-zero:"; echo "$out"; exit 1
  fi
  echo "$out" | grep -q '^TICKET LOOP  backlog-status.sh failed$' \
    || { echo "self-test: a failed run should say what failed:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'AA-04 --claim' && { echo "self-test: a failed run must not print a claim command:"; echo "$out"; exit 1; }

  echo "loop-tui self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  *) render "${REF:-$(default_ref)}" "$(frame_width)" ;;
esac
