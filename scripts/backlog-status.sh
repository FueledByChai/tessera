#!/usr/bin/env bash
# Ticket state derived from git (HK-05). A ticket is done when a commit whose subject starts
# with its id (`HK-01: ...`) is reachable from the ref, the default branch by default; the
# backlog file carries only the claims: no state (or `todo`), `doing` while someone works it,
# `blocked <reason>`. The default branch and the backlog path come from .loop.toml through
# scripts/loop-config.sh (HK-14).
#
#   scripts/backlog-status.sh                 every ticket: id, state, date, sha, blockers, title
#   scripts/backlog-status.sh --next          the id of the first todo whose blockers are done:
#                                             from the `sprint` list in .loop.toml first, in its
#                                             order, then file order (exit 1 when there is none)
#   scripts/backlog-status.sh --sprint        the sprint's tickets in sprint order with their
#                                             states, and a summary line (HK-39)
#   scripts/backlog-status.sh --ref <ref>     commits reachable from <ref> (default: the
#                                             default branch as origin has it, after a fetch,
#                                             so a checkout that has not pulled yet never
#                                             re-offers a merged ticket; the local branch
#                                             when there is no origin or with --local)
#   scripts/backlog-status.sh --backlog <f>   another backlog file (default: the configured one)
#   scripts/backlog-status.sh --local         do not ask origin for ticket/<id> claim branches
#   scripts/backlog-status.sh --self-test     a fixture repo: a `doing` ticket with a landed
#                                             commit reports as done, blockers gate --next, a
#                                             ticket/<id> branch on origin is a claim
#
# A heading reads `### <ID> <title>`, optionally followed by ` — \`<state>\`` and
# ` — Blocked by <ID>, <ID>`. The first commit (oldest) whose subject starts with the id gives
# the date and sha. `done` beats a `doing` or `blocked` claim left behind. A `ticket/<id>`
# branch on origin (scripts/open-ticket-pr.sh --claim) marks the ticket `claimed`: someone is on
# it in another checkout, and --next passes over it (HK-09).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$("$ROOT/scripts/loop-config.sh" default_branch)"
REF_GIVEN=0
BACKLOG="$ROOT/$("$ROOT/scripts/loop-config.sh" backlog)"
MODE=table
LOCAL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --next) MODE=next ;;
    --sprint) MODE=sprint ;;
    --local) LOCAL=1 ;;
    --self-test) MODE=selftest ;;
    --ref) REF="$2"; REF_GIVEN=1; shift ;;
    --backlog) BACKLOG="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"; shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done

# The ref done is judged against, from the current directory's repository: the default branch
# as origin has it (fetched first) unless --ref named one, --local asked for no origin, or
# there is no such remote branch.
judged_ref() {
  local ref="$1"
  if [ "$REF_GIVEN" = 0 ] && [ "$LOCAL" = 0 ] && git rev-parse -q --verify "refs/remotes/origin/$ref" >/dev/null 2>&1; then
    git fetch -q origin "$ref" 2>/dev/null || true
    echo "origin/$ref"
  else
    echo "$ref"
  fi
}

# Prints the table (mode table) or the next ticket id (mode next) for a backlog file against a
# ref, from the current directory's repository.
status() {
  local backlog="$1" ref mode="$3"
  ref="$(judged_ref "$2")"
  local claimed="" sprint
  sprint="$("$ROOT/scripts/loop-config.sh" sprint | tr '\n' ',')"
  if [ "$LOCAL" = 0 ]; then
    # No origin, or one that does not answer, means no claims (and no failure).
    claimed="$( (git ls-remote --heads origin 'refs/heads/ticket/*' 2>/dev/null || true) | sed 's#.*refs/heads/ticket/##' | tr '\n' ',')"
  fi
  git log --reverse --date=short --format='%h %ad %s' "$ref" -- 2>/dev/null \
    | perl -e '
      my ($backlog, $mode, $claimed, $sprint) = @ARGV;
      my %claimed = map { $_ => 1 } grep { length } split /,/, $claimed;
      my @sprint = grep { length } split /,/, $sprint;
      my %sprint; my $pos = 0; $sprint{$_} //= ++$pos for @sprint;
      my %done;
      while (my $line = <STDIN>) {
        chomp $line;
        my ($sha, $date, $subject) = split / /, $line, 3;
        next unless defined $subject && $subject =~ /^([A-Z]+-\d+):/;
        $done{$1} //= [$date, $sha];
      }
      open my $fh, "<", $backlog or die "cannot read $backlog: $!";
      my @tickets;
      while (my $line = <$fh>) {
        chomp $line;
        next unless $line =~ /^### (\S+) (.*)$/;
        my ($id, $rest) = ($1, $2);
        my @parts = split / — /, $rest;
        my $title = shift @parts;
        my ($claim, @blockers) = ("", ());
        for my $part (@parts) {
          if ($part =~ /^`(.*)`$/) { $claim = $1; }
          elsif ($part =~ /^Blocked by (.*)$/) { push @blockers, split /,\s*/, $1; }
        }
        my $state = $done{$id} ? "done"
                  : $claim =~ /^blocked/ ? "blocked"
                  : $claim eq "doing" ? "doing"
                  : $claimed{$id} ? "claimed"
                  : "todo";
        push @tickets, { id => $id, title => $title, state => $state, claim => $claim, blockers => \@blockers };
      }
      close $fh;
      # A blocker is done when its ticket is, or when it has landed and left the file (archived
      # into CHANGELOG.md by scripts/release-notes.sh).
      my %state = map { $_->{id} => $_->{state} } @tickets;
      my $landed = sub { my $b = shift; return ($state{$b} // "") eq "done" || exists $done{$b}; };
      my $ready = sub {
        my $t = shift;
        return 0 unless $t->{state} eq "todo";
        for my $b (@{ $t->{blockers} }) { return 0 unless $landed->($b); }
        return 1;
      };
      my %by_id = map { $_->{id} => $_ } @tickets;
      for my $id (@sprint) { print STDERR "sprint: $id is not in the backlog file\n" unless $by_id{$id}; }
      if ($mode eq "next") {
        for my $id (@sprint) { my $t = $by_id{$id} or next; if ($ready->($t)) { print "$t->{id}\n"; exit 0; } }
        print STDERR "sprint: nothing ready in it; falling back to file order\n" if @sprint;
        for my $t (@tickets) { if ($ready->($t)) { print "$t->{id}\n"; exit 0; } }
        print STDERR "no todo ticket has all its blockers done\n";
        exit 1;
      }
      my @rows = @tickets;
      if ($mode eq "sprint") {
        @rows = grep { defined } map { $by_id{$_} } @sprint;
        if (!@rows) { print "sprint: empty (set sprint = [...] in .loop.toml)\n"; exit 0; }
      }
      printf "%-6s %-8s %-10s %-8s %-6s %-6s %-22s %s\n", "id", "state", "date", "sha", "ready", "sprint", "blocked by", "title";
      for my $t (@rows) {
        my ($date, $sha) = $done{ $t->{id} } ? @{ $done{ $t->{id} } } : ("-", "-");
        my $blockers = join ",", map { $_ . ($landed->($_) ? "" : "!") } @{ $t->{blockers} };
        my $state = $t->{state};
        $state .= " (was $t->{claim})" if $state eq "done" && $t->{claim} ne "" && $t->{claim} ne "todo";
        $state = $t->{claim} if $state eq "blocked";
        printf "%-6s %-8s %-10s %-8s %-6s %-6s %-22s %s\n", $t->{id}, $state, $date, $sha, ($ready->($t) ? "yes" : ""), ($sprint{ $t->{id} } // ""), $blockers, $t->{title};
      }
      if ($mode eq "sprint") {
        my %n; $n{ $_->{state} }++ for @rows;
        my $ready_n = grep { $ready->($_) } @rows;
        printf "sprint: %d ticket(s), %d done, %d ready, %d claimed or doing, %d blocked or waiting\n",
          scalar(@rows), $n{done} // 0, $ready_n, ($n{claimed} // 0) + ($n{doing} // 0),
          scalar(@rows) - ($n{done} // 0) - $ready_n - ($n{claimed} // 0) - ($n{doing} // 0);
      }
    ' "$backlog" "$mode" "$claimed" "$sprint"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/backlog-status.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR"
  (
    cd "$dir"
    git init -q
    git config user.email "self-test@example.com"
    git config user.name "self-test"
    cat > BACKLOG.md <<'EOF'
# Fixture queue

### AA-01 First — `doing`
Body.
**Done when:** it lands.

### AA-02 Second — `todo` — Blocked by AA-01
### AA-03 Third — `blocked waiting on data`
### AA-04 Fourth
### AA-05 Fifth — `todo` — Blocked by AA-03, AA-04
### AA-06 Sixth — `todo` — Blocked by ZZ-09
EOF
    git add BACKLOG.md
    git commit -q -m "Scaffold the fixture queue"
    # A blocker that has landed but is no longer in the file (archived) still counts as done.
    git commit -q --allow-empty -m "ZZ-09: archived long ago"
    "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD | grep -q "^AA-06  todo .* yes .*ZZ-09 " \
      || { echo "self-test: AA-06 should be ready on the archived blocker ZZ-09"; "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD; exit 1; }
    # A ticket/<id> branch on origin is a claim: --next passes over it while it exists, and
    # --local ignores origin altogether.
    git init -q --bare origin.git
    git remote add origin origin.git
    git push -q origin "HEAD:refs/heads/ticket/AA-04"
    "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD | grep -q "^AA-04  claimed" \
      || { echo "self-test: AA-04 should show as claimed by its origin branch"; "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD; exit 1; }
    next="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --next)"
    [ "$next" = "AA-06" ] || { echo "self-test: expected AA-06 next while AA-04 is claimed, got $next"; exit 1; }
    next="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --local --next)"
    [ "$next" = "AA-04" ] || { echo "self-test: --local should ignore the claim and name AA-04, got $next"; exit 1; }
    git push -q origin --delete "ticket/AA-04"
    # Nothing else landed yet: AA-01 is still the doing claim; AA-04 is the first ready todo.
    next="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --next)"
    [ "$next" = "AA-04" ] || { echo "self-test: expected AA-04 next before anything landed, got $next"; exit 1; }
    # A sprint list (HK-39): --next takes the first ready ticket in sprint order before file
    # order, warns about an id the file lacks, and falls back to file order when nothing in
    # the sprint is ready; --sprint shows the sprint's rows in its order with a summary.
    printf '[loop]\nsprint = ["ZZ-99", "AA-05", "AA-06", "AA-04"]\n' > sprint.toml
    out="$(LOOP_CONFIG="$PWD/sprint.toml" "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --next 2>&1)"
    echo "$out" | grep -q '^AA-06$' || { echo "self-test: the sprint should put AA-06 before AA-04, got:"; echo "$out"; exit 1; }
    echo "$out" | grep -q 'sprint: ZZ-99 is not in the backlog file' || { echo "self-test: an unknown sprint id should be reported, got:"; echo "$out"; exit 1; }
    printf '[loop]\nsprint = ["AA-05", "AA-02"]\n' > sprint.toml
    out="$(LOOP_CONFIG="$PWD/sprint.toml" "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --next 2>&1)"
    echo "$out" | grep -q '^AA-04$' || { echo "self-test: an exhausted sprint should fall back to file order (AA-04), got:"; echo "$out"; exit 1; }
    echo "$out" | grep -q 'sprint: nothing ready' || { echo "self-test: the fallback should be announced, got:"; echo "$out"; exit 1; }
    printf '[loop]\nsprint = ["AA-06", "AA-05", "AA-04"]\n' > sprint.toml
    out="$(LOOP_CONFIG="$PWD/sprint.toml" "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --sprint 2>&1)"
    [ "$(echo "$out" | grep -c '^AA-')" = 3 ] || { echo "self-test: --sprint should list the three sprint tickets:"; echo "$out"; exit 1; }
    echo "$out" | sed -n '2p' | grep -q '^AA-06  todo .* yes  *1 ' || { echo "self-test: AA-06 should lead the sprint view at position 1:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^sprint: 3 ticket(s), 0 done, 2 ready, 0 claimed or doing, 1 blocked or waiting$' || { echo "self-test: the sprint summary is off:"; echo "$out"; exit 1; }
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --sprint 2>&1)"
    echo "$out" | grep -q '^sprint: empty' || { echo "self-test: no sprint configured should say so:"; echo "$out"; exit 1; }
    # AA-01 lands while its line still says doing: git wins, with the commit's date and sha.
    git commit -q --allow-empty -m "AA-01: first landed"
    sha="$(git rev-parse --short HEAD)"
    today="$(git log -1 --date=short --format=%ad)"
    table="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD)"
    echo "$table" | grep -q "^AA-01  done (was doing) *$today *$sha" \
      || { echo "self-test: AA-01 should be done on $today at $sha:"; echo "$table"; exit 1; }
    echo "$table" | grep -q "^AA-02  todo .* yes .*AA-01 " \
      || { echo "self-test: AA-02 should be a ready todo once AA-01 landed:"; echo "$table"; exit 1; }
    echo "$table" | grep -q "^AA-03  blocked waiting on data" \
      || { echo "self-test: AA-03 should keep its blocked claim:"; echo "$table"; exit 1; }
    echo "$table" | grep -q "^AA-05  todo .*AA-03!,AA-04!" \
      || { echo "self-test: AA-05 should show both blockers unmet:"; echo "$table"; exit 1; }
    next="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --next)"
    [ "$next" = "AA-02" ] || { echo "self-test: expected AA-02 next, got $next"; exit 1; }
    # A second commit for AA-01 does not move its date; the first one counts.
    git commit -q --allow-empty -m "AA-01: a follow-up fix"
    "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD | grep -q "^AA-01  done (was doing) *$today *$sha" \
      || { echo "self-test: the first AA-01 commit should still date it"; exit 1; }
    # Everything ready landed: --next says so and exits 1.
    git commit -q --allow-empty -m "AA-02: second"
    git commit -q --allow-empty -m "AA-04: fourth"
    git commit -q --allow-empty -m "AA-06: sixth"
    if "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --next 2>/dev/null; then
      echo "self-test: --next should fail with nothing ready (AA-03 is blocked, AA-05 waits on it)"; exit 1
    fi
    # A lagging checkout: AA-03 lands on origin's main through another clone while the local
    # main stays behind. Without --ref the script judges against origin/main after a fetch,
    # so AA-03 is done and AA-05 (blocked by AA-03 and AA-04) is the next ticket; --local
    # keeps judging the local branch, where nothing is ready.
    git branch -q -M main
    git push -q origin main
    git clone -q -b main origin.git peer 2>/dev/null
    (cd peer && git config user.email "peer@example.com" && git config user.name "peer" && git commit -q --allow-empty -m "AA-03: third landed elsewhere" && git push -q origin main)
    "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md | grep -q "^AA-03  done" \
      || { echo "self-test: AA-03 landed on origin/main and should be done though the local main lags"; "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md; exit 1; }
    next="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --next)"
    [ "$next" = "AA-05" ] || { echo "self-test: expected AA-05 next once AA-03 landed on origin, got $next"; exit 1; }
    if "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --local --next 2>/dev/null; then
      echo "self-test: --local should judge the local main, where AA-03 has not landed"; exit 1
    fi
    [ "$(git rev-parse main)" != "$(git rev-parse origin/main)" ] || { echo "self-test: the local main must not have moved"; exit 1; }
  )
  echo "backlog-status self-test passed"
}

# The backlog's own repository answers, so a fixture backlog is judged by its fixture history.
case "$MODE" in
  selftest) self_test ;;
  *) (cd "$(dirname "$BACKLOG")" && status "$BACKLOG" "$REF" "$MODE") ;;
esac
