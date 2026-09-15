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
#   scripts/backlog-status.sh --open [--section <name>]
#                                             the tickets not done and not in the sprint, grouped
#                                             by section, with the story each serves: the pick
#                                             list for the next sprint (HK-40)
#   scripts/backlog-status.sh --show <id>     a ticket's text, state, and the story it serves, or
#                                             a story's text, derived status, and its tickets
#   scripts/backlog-status.sh --stories       every story in the product backlog (`stories` in
#                                             .loop.toml) with a status derived from git: done
#                                             when every ticket that serves it landed, open k/n,
#                                             or unticketed
#   scripts/backlog-status.sh --ref <ref>     commits reachable from <ref> (default: the
#                                             default branch as origin has it, after a fetch,
#                                             so a checkout that has not pulled yet never
#                                             re-offers a merged ticket; the local branch
#                                             when there is no origin or with --local)
#   scripts/backlog-status.sh --backlog <f>   another backlog file (default: the configured one)
#   scripts/backlog-status.sh --local         do not ask origin for ticket/<id> claim branches
#   scripts/backlog-status.sh --self-test     a fixture repo judged by its own settings: a
#                                             `doing` ticket with a landed commit reports as
#                                             done, blockers gate --next, a ticket/<id> branch
#                                             on origin is a claim
#
# A heading reads `### <ID> <title>`, optionally followed by ` — \`<state>\`` and
# ` — Blocked by <ID>, <ID>`. The first commit (oldest) whose subject starts with the id gives
# the date and sha. `done` beats a `doing` or `blocked` claim left behind. A `ticket/<id>`
# branch on origin (scripts/open-ticket-pr.sh --claim) marks the ticket `claimed`: someone is on
# it in another checkout, and --next passes over it (HK-09).
#
# Tickets that have left the file still count. scripts/release-notes.sh --archive moves shipped
# tickets into `CHANGELOG.md` at the repository root as `#### <id> <title> — <date> · <sha>`
# blocks, and those blocks carry the ticket's text with them, so their `Serves` lines are read
# back: a story whose tickets have all been archived reads done rather than unticketed, and
# --show <story> lists them as done with the date they were archived. A missing changelog is
# simply no archived tickets. The changelog is the working tree's, so an archived ticket counts
# as done only when the commit its block names is reachable from the ref being judged: at a ref
# before that commit the ticket had not shipped, and marking it done reports a story finished
# at a ref where it was not.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="$("$ROOT/scripts/loop-config.sh" default_branch)"
REF_GIVEN=0
BACKLOG="$ROOT/$("$ROOT/scripts/loop-config.sh" backlog)"
MODE=table
LOCAL=0
WANT=""
SECTION=""
STORIES_FILE="$("$ROOT/scripts/loop-config.sh" stories)"
while [ $# -gt 0 ]; do
  case "$1" in
    --next) MODE=next ;;
    --sprint) MODE=sprint ;;
    --open) MODE=open ;;
    --section) SECTION="$2"; shift ;;
    --show) MODE=show; WANT="$2"; shift ;;
    --stories) MODE=stories ;;
    --stories-file) STORIES_FILE="$2"; shift ;;
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
  local claimed="" sprint stories="" changelog=""
  sprint="$("$ROOT/scripts/loop-config.sh" sprint | tr '\n' ',')"
  # The product backlog sits beside the ticket file's root; a relative path is resolved from
  # the ticket file's repository root, an absolute one (--stories-file) as given.
  if [ -n "$STORIES_FILE" ]; then
    case "$STORIES_FILE" in /*) stories="$STORIES_FILE" ;; *) stories="$(git rev-parse --show-toplevel 2>/dev/null || pwd)/$STORIES_FILE" ;; esac
  fi
  # The changelog --archive writes sits at that same root; a missing one is no archived tickets.
  changelog="$(git rev-parse --show-toplevel 2>/dev/null || pwd)/CHANGELOG.md"
  if [ "$LOCAL" = 0 ]; then
    # No origin, or one that does not answer, means no claims (and no failure).
    claimed="$( (git ls-remote --heads origin 'refs/heads/ticket/*' 2>/dev/null || true) | sed 's#.*refs/heads/ticket/##' | tr '\n' ',')"
  fi
  git log --reverse --date=short --format='%h %ad %s' "$ref" -- 2>/dev/null \
    | perl -e '
      my ($backlog, $mode, $claimed, $sprint, $stories_file, $changelog, $want, $section_filter, $judged_ref) = @ARGV;
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
      # The ticket file: `## <section>` headings, `### <ID> <title>` tickets with their claim and
      # blockers, and the body of each ticket (for --show and the story it serves).
      open my $fh, "<", $backlog or die "cannot read $backlog: $!";
      my (@tickets, $section, $cur);
      $section = "";
      while (my $line = <$fh>) {
        chomp $line;
        if ($line =~ /^## (.*)$/) { $section = $1; $cur = undef; next; }
        if ($line =~ /^### (\S+) (.*)$/) {
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
          $cur = { id => $id, title => $title, state => $state, claim => $claim, blockers => \@blockers,
                   section => $section, heading => $line, body => [], serves => [] };
          push @tickets, $cur;
          next;
        }
        push @{ $cur->{body} }, $line if $cur;
      }
      close $fh;
      for my $t (@tickets) {
        my $text = join " ", @{ $t->{body} };
        while ($text =~ /\bserves\s+([A-Z]+-\d+(?:\s*,\s*[A-Z]+-\d+)*)/gi) { push @{ $t->{serves} }, split /\s*,\s*/, $1; }
      }
      # Every commit reachable from the ref being judged, so an archived ticket can be judged
      # by whether its own commit is one of them. `git rev-list` failing (an unresolvable ref)
      # leaves the list empty, and then nothing is judged and the changelog is taken at its word
      # rather than every archived ticket turning todo.
      my @reachable = split /\n/, `git rev-list "$judged_ref" 2>/dev/null`;
      my %reachable;
      my $landed_archived = sub {
        my $sha = shift;
        return 1 unless @reachable;
        return $reachable{$sha} //= (grep { index($_, $sha) == 0 } @reachable) ? 1 : 0;
      };
      # Tickets that have left the file. release-notes.sh --archive writes each as
      # `#### <id> <title> — <date> · <sha>` with its own text underneath, so the `Serves` lines
      # come back with them; a heading ends the block, and a missing changelog is no archived
      # tickets. They are kept out of @tickets so the tables and --open stay the file contents.
      # A block whose commit the judged ref does not reach is a ticket that had not shipped yet
      # there, so it reads todo: the story it serves was still open at that ref.
      my @archived;
      if ($changelog ne "" && open my $cf, "<", $changelog) {
        my $a;
        while (my $line = <$cf>) {
          chomp $line;
          if ($line =~ /^#### ([A-Z]+-\d+) (.*?) — (\d{4}-\d{2}-\d{2}) · ([0-9a-f]{4,40})/) {
            $a = { id => $1, title => $2, date => $3, sha => $4,
                   state => ($landed_archived->($4) ? "done" : "todo"), claim => "",
                   archived => 1, section => "", heading => $line, body => [], serves => [] };
            push @archived, $a; next;
          }
          if ($line =~ /^#/) { $a = undef; next; }
          push @{ $a->{body} }, $line if $a;
        }
        close $cf;
        for my $a (@archived) {
          my $text = join " ", @{ $a->{body} };
          while ($text =~ /\bserves\s+([A-Z]+-\d+(?:\s*,\s*[A-Z]+-\d+)*)/gi) { push @{ $a->{serves} }, split /\s*,\s*/, $1; }
        }
      }
      # The product backlog, when the project keeps one: `## <epic>` and `### <ID> — <title>`
      # stories with their body; the status of a story is derived from the tickets that serve it.
      my (@stories, %story);
      if ($stories_file ne "" && open my $sf, "<", $stories_file) {
        my ($epic, $s) = ("", undef);
        while (my $line = <$sf>) {
          chomp $line;
          if ($line =~ /^## (.*)$/) { $epic = $1; $s = undef; next; }
          if ($line =~ /^### ([A-Z]+-\d+) — (.*)$/) {
            $s = { id => $1, title => $2, epic => $epic, written => "", heading => $line, body => [], tickets => [] };
            push @stories, $s; $story{$1} = $s; next;
          }
          next unless $s;
          $s->{written} = $1 if $line =~ /^\*\*Status:\*\*\s*(.*?)\s*$/;
          push @{ $s->{body} }, $line;
        }
        close $sf;
        for my $t (@tickets, @archived) { for my $id (@{ $t->{serves} }) { push @{ $story{$id}{tickets} }, $t if $story{$id}; } }
      }
      my $story_status = sub {
        my $s = shift;
        my @t = @{ $s->{tickets} };
        return "unticketed" unless @t;
        my $d = grep { $_->{state} eq "done" } @t;
        return $d == @t ? "done" : "open $d/" . scalar(@t);
      };
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
      my $blockers_of = sub { my $t = shift; join ",", map { $_ . ($landed->($_) ? "" : "!") } @{ $t->{blockers} }; };
      my $state_of = sub {
        my $t = shift; my $state = $t->{state};
        $state .= " (was $t->{claim})" if $state eq "done" && $t->{claim} ne "" && $t->{claim} ne "todo";
        $state .= " (archived $t->{date})" if $t->{archived};
        $state = $t->{claim} if $state eq "blocked";
        return $state;
      };
      if ($mode eq "next") {
        for my $id (@sprint) { my $t = $by_id{$id} or next; if ($ready->($t)) { print "$t->{id}\n"; exit 0; } }
        print STDERR "sprint: nothing ready in it; falling back to file order\n" if @sprint;
        for my $t (@tickets) { if ($ready->($t)) { print "$t->{id}\n"; exit 0; } }
        print STDERR "no todo ticket has all its blockers done\n";
        exit 1;
      }
      if ($mode eq "show") {
        if (my $t = $by_id{$want}) {
          print "$t->{heading}\n", map { "$_\n" } @{ $t->{body} };
          printf "state: %s%s%s\n", $state_of->($t), ($ready->($t) ? ", ready" : ""), ($sprint{$want} ? ", sprint position $sprint{$want}" : "");
          for my $id (@{ $t->{serves} }) {
            my $s = $story{$id};
            my $st = $s ? $story_status->($s) : "";
            printf "serves: %s%s\n", $id, $s ? " — $s->{title} ($st)" : " (not in the product backlog)";
          }
          exit 0;
        }
        if (my $s = $story{$want}) {
          print "$s->{heading}\n", map { "$_\n" } @{ $s->{body} };
          printf "status: %s (the file says: %s)\n", $story_status->($s), ($s->{written} || "nothing");
          printf "ticket: %-6s %-8s %s\n", $_->{id}, $state_of->($_), $_->{title} for @{ $s->{tickets} };
          exit 0;
        }
        print STDERR "$want: no such ticket or story\n";
        exit 1;
      }
      if ($mode eq "stories") {
        if (!@stories) { print STDERR "no product backlog" . ($stories_file ne "" ? " at $stories_file" : " configured (stories in .loop.toml)") . "\n"; exit 1; }
        printf "%-8s %-11s %-8s %-30s %s\n", "id", "status", "tickets", "epic", "title";
        for my $s (@stories) {
          my $epic = $s->{epic}; $epic = substr($epic, 0, 29) . "…" if length $epic > 30;
          my @ids = map { $_->{id} } @{ $s->{tickets} };
          printf "%-8s %-11s %-8s %-30s %s\n", $s->{id}, $story_status->($s), (@ids ? scalar(@ids) : "-"), $epic, $s->{title};
        }
        my %n; $n{ ($story_status->($_) =~ /^(\w+)/)[0] }++ for @stories;
        printf "stories: %d, %d done, %d open, %d unticketed\n", scalar(@stories), $n{done} // 0, $n{open} // 0, $n{unticketed} // 0;
        exit 0;
      }
      if ($mode eq "open") {
        my $last = "\0"; my $n = 0;
        for my $t (@tickets) {
          next if $t->{state} eq "done" || $sprint{ $t->{id} };
          next if $section_filter ne "" && index(lc $t->{section}, lc $section_filter) < 0;
          if ($t->{section} ne $last) { print "## $t->{section}\n"; $last = $t->{section}; }
          my $serves = join ",", @{ $t->{serves} };
          printf "%-6s %-8s %-6s %-14s %-10s %s\n", $t->{id}, $state_of->($t), ($ready->($t) ? "ready" : ""), $blockers_of->($t), $serves, $t->{title};
          $n++;
        }
        print "open: $n ticket(s) not done and not in the sprint", (@sprint ? " (sprint: " . join(", ", @sprint) . ")" : " (no sprint set)"), "\n";
        exit 0;
      }
      my @rows = @tickets;
      if ($mode eq "sprint") {
        @rows = grep { defined } map { $by_id{$_} } @sprint;
        if (!@rows) { print "sprint: empty (set sprint = [...] in .loop.toml)\n"; exit 0; }
      }
      printf "%-6s %-8s %-10s %-8s %-6s %-6s %-22s %s\n", "id", "state", "date", "sha", "ready", "sprint", "blocked by", "title";
      for my $t (@rows) {
        my ($date, $sha) = $done{ $t->{id} } ? @{ $done{ $t->{id} } } : ("-", "-");
        printf "%-6s %-8s %-10s %-8s %-6s %-6s %-22s %s\n", $t->{id}, $state_of->($t), $date, $sha, ($ready->($t) ? "yes" : ""), ($sprint{ $t->{id} } // ""), $blockers_of->($t), $t->{title};
      }
      if ($mode eq "sprint") {
        my %n; $n{ $_->{state} }++ for @rows;
        my $ready_n = grep { $ready->($_) } @rows;
        printf "sprint: %d ticket(s), %d done, %d ready, %d claimed or doing, %d blocked or waiting\n",
          scalar(@rows), $n{done} // 0, $ready_n, ($n{claimed} // 0) + ($n{doing} // 0),
          scalar(@rows) - ($n{done} // 0) - $ready_n - ($n{claimed} // 0) - ($n{doing} // 0);
      }
    ' "$backlog" "$mode" "$claimed" "$sprint" "$stories" "$changelog" "$WANT" "$SECTION" "$ref"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/backlog-status.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR"
  # The fixture is judged by its own settings, never the checkout's: every call below resolves
  # its config here, so this test reads and prints the same wherever it runs (LK-10). No sprint
  # list, so --next falls back to file order and --sprint says the sprint is empty.
  printf '[loop]\ndefault_branch = "main"\n' > "$dir/.loop.toml"
  export LOOP_CONFIG="$dir/.loop.toml"
  (
    cd "$dir"
    # Anything the fixture writes to stderr is a call that reached for settings other than its
    # own; the guard at the end turns that into a failure rather than the noise a real mismatch
    # would be lost in.
    exec 2>"$dir/.stderr"
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
    # Stories (HK-40): a product backlog whose stories the tickets serve. AA-01 and AA-02 serve
    # BT-1, AA-05 serves BT-2, nothing serves BT-3. Nothing has landed yet.
    cat > PRODUCT.md <<'EOF2'
# Product backlog

## Epic A: Alpha things

### BT-1 — Two tickets serve this

**Status:** Proposed
**User story:** As a tester, I want two tickets, so that the status is derived.

**Acceptance criteria:**

- Both land.

### BT-2 — One blocked ticket serves this

**Status:** Proposed

## Epic B: Beta things

### BT-3 — Nothing serves this yet

**Status:** Proposed
EOF2
    printf '# Fixture queue\n\n## Alpha\n\n### AA-01 First — `doing`\nBody. Serves BT-1.\n**Done when:** it lands.\n\n### AA-02 Second — `todo` — Blocked by AA-01\nSecond body, which serves\nBT-1 across a line break.\n### AA-03 Third — `blocked waiting on data`\n\n## Beta\n\n### AA-04 Fourth\n### AA-05 Fifth — `todo` — Blocked by AA-03, AA-04\nServes BT-2.\n### AA-06 Sixth — `todo` — Blocked by ZZ-09\n' > BACKLOG.md
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --stories 2>&1)"
    echo "$out" | grep -q '^BT-1 *open 0/2 *2 ' || { echo "self-test: BT-1 should be open 0/2:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^BT-3 *unticketed *- ' || { echo "self-test: BT-3 should be unticketed:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^stories: 3, 0 done, 2 open, 1 unticketed$' || { echo "self-test: the stories summary is off:"; echo "$out"; exit 1; }
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --show AA-02 2>&1)"
    echo "$out" | grep -q '^### AA-02 Second' && echo "$out" | grep -q '^Second body' && echo "$out" | grep -q '^state: todo$' && echo "$out" | grep -q '^serves: BT-1 — Two tickets serve this (open 0/2)$' \
      || { echo "self-test: --show AA-02 should print its text, state, and story:"; echo "$out"; exit 1; }
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --show BT-1 2>&1)"
    echo "$out" | grep -q '^### BT-1 — Two tickets' && echo "$out" | grep -q '^status: open 0/2 (the file says: Proposed)$' && echo "$out" | grep -q '^ticket: AA-02  todo ' \
      || { echo "self-test: --show BT-1 should print its text, derived status, and tickets:"; echo "$out"; exit 1; }
    if "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --show AA-99 >/dev/null 2>&1; then echo "self-test: --show of an unknown id must fail"; exit 1; fi
    printf '[loop]\nsprint = ["AA-04"]\n' > sprint.toml
    out="$(LOOP_CONFIG="$PWD/sprint.toml" "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --open 2>&1)"
    echo "$out" | grep -q '^## Alpha$' && echo "$out" | grep -q '^## Beta$' || { echo "self-test: --open should group by section:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^AA-04 ' && { echo "self-test: --open must leave out the sprint's tickets:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^AA-05  todo .*AA-03!,AA-04! *BT-2 ' || { echo "self-test: --open should show AA-05 with its blockers and story:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^open: 5 ticket(s) not done and not in the sprint (sprint: AA-04)$' || { echo "self-test: the open summary is off:"; echo "$out"; exit 1; }
    out="$(LOOP_CONFIG="$PWD/sprint.toml" "$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --open --section beta 2>&1)"
    [ "$(echo "$out" | grep -c '^AA-')" = 2 ] || { echo "self-test: --section beta should list two tickets:"; echo "$out"; exit 1; }
    # AA-01 and AA-02 land: BT-1 is done, and --open no longer lists them.
    git commit -q --allow-empty -m "AA-01: first" && git commit -q --allow-empty -m "AA-02: second"
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --stories 2>&1)"
    echo "$out" | grep -q '^BT-1 *done ' || { echo "self-test: BT-1 should be done once both tickets landed:"; echo "$out"; exit 1; }
    # LK-08: archiving a release moves those tickets into CHANGELOG.md, and the story they served
    # is still done - the archived blocks carry their Serves lines with them.
    "$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --archive v0.1.0 HEAD~2 HEAD >/dev/null
    grep -q '^#### AA-01 ' CHANGELOG.md || { echo "self-test: the archive should have written AA-01:"; cat CHANGELOG.md; exit 1; }
    if grep -q '^### AA-01 ' BACKLOG.md; then echo "self-test: AA-01 should have left the backlog"; exit 1; fi
    archive_date="$(grep '^#### AA-01 ' CHANGELOG.md | grep -oE '[0-9]{4}-[0-9]{2}-[0-9]{2}' | head -1)"
    [ -n "$archive_date" ] || { echo "self-test: the AA-01 block should carry its archive date:"; grep '^#### AA-01 ' CHANGELOG.md; exit 1; }
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --stories 2>&1)"
    echo "$out" | grep -q '^BT-1 *done *2 ' || { echo "self-test: BT-1 should still be done on its archived tickets:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '^stories: 3, 1 done, 1 open, 1 unticketed$' || { echo "self-test: the stories summary after archiving is off:"; echo "$out"; exit 1; }
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD --stories-file "$PWD/PRODUCT.md" --show BT-1 2>&1)"
    echo "$out" | grep -q "^ticket: AA-01  done (archived $archive_date) " || { echo "self-test: --show BT-1 should list AA-01 as done with its archive date:"; echo "$out"; exit 1; }
    echo "$out" | grep -q "^ticket: AA-02  done (archived $archive_date) " || { echo "self-test: --show BT-1 should list AA-02 the same way:"; echo "$out"; exit 1; }
    # The changelog is the working tree's, so at a ref before the commits it names the tickets
    # had not shipped: they must not read done, and the story they serve must not either.
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD~2 --stories-file "$PWD/PRODUCT.md" --stories 2>&1)"
    echo "$out" | grep -q '^BT-1 *open 0/2 ' || { echo "self-test: BT-1 must not read done at a ref before its archived tickets landed:"; echo "$out"; exit 1; }
    out="$("$ROOT/scripts/backlog-status.sh" --backlog BACKLOG.md --ref HEAD~2 --stories-file "$PWD/PRODUCT.md" --show BT-1 2>&1)"
    echo "$out" | grep -q "^ticket: AA-01  todo " || { echo "self-test: --show BT-1 at HEAD~2 should list AA-01 as todo:"; echo "$out"; exit 1; }
    echo "$out" | grep -q "^ticket: AA-02  todo " || { echo "self-test: --show BT-1 at HEAD~2 should list AA-02 as todo:"; echo "$out"; exit 1; }
    git reset -q --hard HEAD~2
    git checkout -q -- BACKLOG.md; rm -f PRODUCT.md sprint.toml CHANGELOG.md
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
    if [ -s "$dir/.stderr" ]; then
      echo "self-test: the fixture wrote to stderr, so its output depends on the checkout it runs in:"
      sed 's/^/  /' "$dir/.stderr"
      exit 1
    fi
    echo "backlog-status self-test passed"
  )
  unset LOOP_CONFIG
}

# The backlog's own repository answers, so a fixture backlog is judged by its fixture history.
case "$MODE" in
  selftest) self_test ;;
  *) (cd "$(dirname "$BACKLOG")" && status "$BACKLOG" "$REF" "$MODE") ;;
esac
