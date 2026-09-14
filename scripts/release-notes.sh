#!/usr/bin/env bash
# Release notes from commits, and a changelog archive (HK-06). A commit whose subject starts
# with a ticket id (`WB-02: ...`) is that ticket shipping; the notes are those commits between
# two refs, grouped by ticket prefix under the BACKLOG.md section the ticket sits in.
#
#   scripts/release-notes.sh <from-ref> [<to-ref>]     Markdown notes for from..to (to: the
#                                                      default branch from .loop.toml)
#   scripts/release-notes.sh --archive <tag> <from> [<to>]
#                                                      the notes, plus: every listed ticket
#                                                      still in BACKLOG.md moves, body and all,
#                                                      into CHANGELOG.md under a `## <tag>`
#                                                      heading (newest first), so the queue
#                                                      holds only open work
#   --backlog <file> / --changelog <file>              other files (default: the repo's)
#   --prefix <P>                                       only the tickets whose id carries that
#                                                      prefix (repeatable, or comma-separated);
#                                                      it filters the notes and the archive
#                                                      alike, so one prefix's work can leave a
#                                                      backlog without touching the rest. When
#                                                      nothing matches, nothing moves and the
#                                                      exit status is still 0. A --prefix that
#                                                      names no prefix at all is refused, since
#                                                      read as "no filter" it would archive
#                                                      every prefix's tickets.
#   --self-test                                        a fixture repo proves both modes
#
# Tag releases; the notes for a release are the diff between its tag and the previous one.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKLOG="$ROOT/$("$ROOT/scripts/loop-config.sh" backlog)"
CHANGELOG="$ROOT/CHANGELOG.md"
ARCHIVE=""
MODE=notes
REFS=()
PREFIXES=""
PREFIX_GIVEN=0
# --prefix accumulates, splitting on commas, so `--prefix AA --prefix BB` and `--prefix AA,BB`
# mean the same thing. An empty component is dropped: it names no prefix, and dropping it cannot
# widen the filter.
add_prefix() {
  local list="$1" p
  local IFS=','
  for p in $list; do
    [ -n "$p" ] || continue
    PREFIXES="${PREFIXES:+$PREFIXES,}$p"
  done
}
while [ $# -gt 0 ]; do
  case "$1" in
    --archive) ARCHIVE="$2"; shift ;;
    --backlog) BACKLOG="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"; shift ;;
    --changelog) CHANGELOG="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"; shift ;;
    --prefix) PREFIX_GIVEN=1; add_prefix "$2"; shift ;;
    --self-test) MODE=selftest ;;
    --*) echo "unknown flag: $1" >&2; exit 2 ;;
    *) REFS+=("$1") ;;
  esac
  shift
done
# A --prefix that was asked for and named nothing is a mistake, not an omission. Read as "no
# filter" it would archive every prefix's tickets, which is the opposite of what it asked for, so
# it is refused before either file is touched.
if [ "$PREFIX_GIVEN" = 1 ] && [ -z "$PREFIXES" ]; then
  echo "usage: --prefix needs at least one prefix; an empty list is not the same as leaving the flag off" >&2
  exit 2
fi
# The filter reaches the renderer through the environment: the perl block below is a black box
# that already takes its inputs positionally, and the sibling TUI script passes its width the
# same way.
RELEASE_NOTES_PREFIXES="$PREFIXES"
export RELEASE_NOTES_PREFIXES

# The commits from..to that carry a ticket id, oldest first, as `sha<TAB>date<TAB>id<TAB>summary`.
shipped() {
  local from="$1" to="$2"
  git log --reverse --date=short --format='%h%x09%ad%x09%s' "$from..$to" -- 2>/dev/null \
    | perl -ne 'chomp; my ($sha, $date, $subject) = split /\t/, $_, 3; next unless $subject =~ /^([A-Z]+-\d+):\s*(.*)$/; print "$sha\t$date\t$1\t$2\n";'
}

# Notes (mode notes) or the archive (mode archive with a tag): reads the shipped list on STDIN.
render() {
  local mode="$1" tag="$2" from="$3" to="$4" backlog="$5" changelog="$6"
  perl -e '
    use strict; use warnings;
    my ($mode, $tag, $from, $to, $backlog, $changelog) = @ARGV;
    # --prefix narrows the run to the tickets whose id carries one of these prefixes. An empty
    # list means no filter, which is what a flag-less invocation passes.
    my %want_prefix = map { $_ => 1 } grep { length } split /,/, ($ENV{RELEASE_NOTES_PREFIXES} // "");
    my $keep = sub {
      my $id = shift;
      return 1 unless %want_prefix;
      my ($p) = $id =~ /^([A-Z]+)-/;
      return (defined $p && $want_prefix{$p}) ? 1 : 0;
    };
    my @shipped;
    while (my $line = <STDIN>) { chomp $line; my @f = split /\t/, $line, 4; push @shipped, { sha => $f[0], date => $f[1], id => $f[2], summary => $f[3] } if @f == 4; }
    # The backlog: every ticket block (heading through the line before the next heading) and
    # the section it sits in.
    my (@lines, %block, %section, %order);
    if (open my $fh, "<", $backlog) { @lines = <$fh>; close $fh; }
    my ($current_section, $current_id) = ("", "");
    for my $i (0 .. $#lines) {
      my $line = $lines[$i];
      if ($line =~ /^## (.*)$/) { $current_section = $1; $current_id = ""; next; }
      if ($line =~ /^### (\S+) /) { $current_id = $1; $section{$current_id} = $current_section; $block{$current_id} = [$i, $i]; $order{$current_id} = $i; next; }
      $block{$current_id}[1] = $i if $current_id ne "";
    }
    # The filter is applied once, here, so the notes, the archive, and the "already archived"
    # line all see the same tickets.
    my @ships = grep { $keep->($_->{id}) } @shipped;
    my %seen;
    my @ids = grep { !$seen{$_}++ } map { $_->{id} } @ships;
    my %first; for my $s (@ships) { $first{ $s->{id} } //= $s; }
    my %by_prefix;
    for my $id (@ids) { my ($prefix) = $id =~ /^([A-Z]+)-/; push @{ $by_prefix{$prefix} }, $id; }
    my $section_name = sub { my $prefix = shift; for my $id (@{ $by_prefix{$prefix} }) { return "$section{$id} ($prefix)" if ($section{$id} // "") ne ""; } return $prefix; };
    my $title = sub { my $id = shift; return "" unless $block{$id}; my $h = $lines[ $block{$id}[0] ]; $h =~ s/^### \S+ //; $h =~ s/ — .*$//; chomp $h; return $h; };
    my $notes = "";
    for my $prefix (sort keys %by_prefix) {
      $notes .= "\n### " . $section_name->($prefix) . "\n\n";
      for my $id (@{ $by_prefix{$prefix} }) {
        my $f = $first{$id};
        my $t = $title->($id);
        $notes .= "- **$id** " . ($t ne "" ? "$t" : $f->{summary}) . " — $f->{date} · $f->{sha}" . ($t ne "" && lc($t) ne lc($f->{summary}) ? " ($f->{summary})" : "") . "\n";
      }
    }
    my $scope = %want_prefix ? " matching " . join(", ", sort keys %want_prefix) : "";
    if ($mode eq "notes") {
      print "# Release notes $from..$to\n";
      print @ids ? $notes : "\nNo ticket commits in $from..$to$scope.\n";
      exit 0;
    }
    # A filter that matches nothing leaves both files exactly as they were, and is not an error:
    # the prefix may simply have had no work in this range.
    if (%want_prefix && !@ids) {
      print "nothing in $from..$to$scope; nothing archived\n";
      exit 0;
    }
    # Archive: the tickets still in the backlog move into the changelog under the tag.
    my $existing = "";
    if (open my $ch, "<", $changelog) { local $/; $existing = <$ch>; close $ch; }
    my @moved = grep { $block{$_} && $existing !~ /^#### \Q$_\E /m } @ids;
    my $date = @ships ? $ships[-1]{date} : "";
    my $entry = "## $tag — $date ($from..$to)\n" . $notes;
    for my $prefix (sort keys %by_prefix) {
      my @here = grep { $block{$_} } @{ $by_prefix{$prefix} };
      next unless @here;
      $entry .= "\n### " . $section_name->($prefix) . ": archived tickets\n";
      for my $id (@here) {
        my ($start, $end) = @{ $block{$id} };
        my @body = @lines[$start .. $end];
        $body[0] =~ s{^### \S+ (.*?)(?: — `[^`]*`)?( — Blocked by .*)?$}{"#### $id $1 — $first{$id}{date} · $first{$id}{sha}" . (defined $2 ? $2 : "")}e;
        pop @body while @body > 1 && $body[-1] =~ /^\s*$/;
        $entry .= "\n" . join("", @body);
      }
    }
    $entry .= "\n";
    my $header = "# Changelog\n\nWhat shipped, by release: the commits that carry a ticket id between two tags, with the\nticket text as it stood when it left BACKLOG.md (scripts/release-notes.sh --archive).\n\n";
    if ($existing =~ /^## \Q$tag\E /m) {
      print "changelog already has a $tag entry; leaving it as it is\n";
    } else {
      my $out;
      if ($existing =~ /^(# Changelog\n(?:.*\n)*?)(?=^## |\z)/m) { my $head = $1; my $rest = substr $existing, length $head; $out = $head . $entry . $rest; }
      else { $out = $header . $entry . $existing; }
      open my $wc, ">", $changelog or die "cannot write $changelog: $!"; print $wc $out; close $wc;
    }
    # Drop the moved blocks (and the blank lines that trailed them) from the backlog.
    my %drop;
    for my $id (@moved) { my ($start, $end) = @{ $block{$id} }; $drop{$_} = 1 for $start .. $end; }
    my @kept;
    for my $i (0 .. $#lines) { push @kept, $lines[$i] unless $drop{$i}; }
    my $text = join "", @kept; $text =~ s/\n{3,}/\n\n/g;
    open my $wb, ">", $backlog or die "cannot write $backlog: $!"; print $wb $text; close $wb;
    print "archived " . scalar(@moved) . " ticket(s) under $tag: " . join(", ", @moved) . "\n";
    my @already = grep { !$block{$_} } @ids; print "already archived or never in the backlog: " . join(", ", @already) . "\n" if @already;
  ' "$mode" "$tag" "$from" "$to" "$backlog" "$changelog"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/release-notes.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR"
  (
    cd "$dir"
    git init -q
    git config user.email "self-test@example.com"
    git config user.name "self-test"
    cat > BACKLOG.md <<'EOF'
# Fixture queue

## Alpha

### AA-01 First thing — `doing`
Why the first thing matters.
**Done when:** it lands.

### AA-02 Second thing — `todo` — Blocked by AA-01
Body of the second thing.

## Beta

### BB-01 Other thing
Body of the other thing.
**Done when:** it ships.

## Gamma

### CC-01 Third thing
Body of the third thing.
EOF
    git add BACKLOG.md
    git commit -q -m "Scaffold the fixture queue"
    git tag v0.0.0
    git commit -q --allow-empty -m "AA-01: first thing landed"
    git commit -q --allow-empty -m "Unrelated tidy-up"
    git commit -q --allow-empty -m "CC-01: third thing shipped"
    git commit -q --allow-empty -m "BB-01: other thing shipped"
    git commit -q --allow-empty -m "AA-01: a follow-up fix"
    notes="$("$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md v0.0.0 HEAD)"
    echo "$notes" | grep -q '^### Alpha (AA)$' || { echo "self-test: notes lack the Alpha section:"; echo "$notes"; exit 1; }
    echo "$notes" | grep -q '^- \*\*AA-01\*\* First thing — [0-9-]* · [0-9a-f]* (first thing landed)$' || { echo "self-test: AA-01 line wrong:"; echo "$notes"; exit 1; }
    echo "$notes" | grep -q '^### Beta (BB)$' || { echo "self-test: notes lack the Beta section:"; echo "$notes"; exit 1; }
    echo "$notes" | grep -q '^- \*\*BB-01\*\* Other thing' || { echo "self-test: BB-01 missing:"; echo "$notes"; exit 1; }
    echo "$notes" | grep -q 'AA-02' && { echo "self-test: AA-02 has not shipped and must not be listed:"; echo "$notes"; exit 1; }
    [ "$(echo "$notes" | grep -c 'AA-01')" = 1 ] || { echo "self-test: AA-01 must be listed once (first commit):"; echo "$notes"; exit 1; }
    echo "$notes" | grep -q 'Unrelated' && { echo "self-test: a commit without a ticket id leaked in"; exit 1; }
    # --prefix narrows the notes to one prefix, and three prefixes interleave in this range, so
    # the two it does not name must be absent from both the list and the section headings.
    notes_aa="$("$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --prefix AA v0.0.0 HEAD)"
    echo "$notes_aa" | grep -q '^- \*\*AA-01\*\*' || { echo "self-test: --prefix AA dropped AA-01:"; echo "$notes_aa"; exit 1; }
    echo "$notes_aa" | grep -qE 'BB-01|CC-01' && { echo "self-test: --prefix AA leaked another prefix:"; echo "$notes_aa"; exit 1; }
    echo "$notes_aa" | grep -q '^### Alpha (AA)$' || { echo "self-test: --prefix AA lost its own section:"; echo "$notes_aa"; exit 1; }
    echo "$notes_aa" | grep -qE '^### (Beta|Gamma)' && { echo "self-test: --prefix AA kept another prefix's section:"; echo "$notes_aa"; exit 1; }
    notes_bb="$("$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --prefix BB v0.0.0 HEAD)"
    echo "$notes_bb" | grep -q '^- \*\*BB-01\*\*' || { echo "self-test: --prefix BB dropped BB-01:"; echo "$notes_bb"; exit 1; }
    echo "$notes_bb" | grep -qE 'AA-01|CC-01' && { echo "self-test: --prefix BB leaked another prefix:"; echo "$notes_bb"; exit 1; }
    # Both spellings of the flag mean the same thing, and together they mean no filter at all.
    [ "$("$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --prefix AA,BB,CC v0.0.0 HEAD)" = "$notes" ] \
      || { echo "self-test: a comma-separated --prefix list must equal no filter"; exit 1; }
    [ "$("$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --prefix AA --prefix BB --prefix CC v0.0.0 HEAD)" = "$notes" ] \
      || { echo "self-test: a repeated --prefix must equal no filter"; exit 1; }
    # The archive moves exactly the prefix it was given, says how many it moved, and leaves the
    # other prefixes' tickets where they were.
    archive_out="$("$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --prefix AA --archive v0.1.0 v0.0.0 HEAD)"
    [ "$archive_out" = "archived 1 ticket(s) under v0.1.0: AA-01" ] || { echo "self-test: --prefix AA must move one ticket and print the count:"; echo "$archive_out"; exit 1; }
    grep -q '^#### AA-01 First thing' CHANGELOG.md || { echo "self-test: --prefix AA did not archive AA-01:"; cat CHANGELOG.md; exit 1; }
    grep -qE 'BB-01|CC-01' CHANGELOG.md && { echo "self-test: --prefix AA archived another prefix:"; cat CHANGELOG.md; exit 1; }
    grep -q '^### AA-01 ' BACKLOG.md && { echo "self-test: AA-01 still in the backlog after the prefix archive"; exit 1; }
    grep -q '^### BB-01 Other thing$' BACKLOG.md || { echo "self-test: --prefix AA removed BB-01:"; cat BACKLOG.md; exit 1; }
    grep -q '^### CC-01 Third thing$' BACKLOG.md || { echo "self-test: --prefix AA removed CC-01:"; cat BACKLOG.md; exit 1; }
    # A prefix with nothing in the range moves nothing, changes nothing, and is not an error.
    changelog_before="$(cat CHANGELOG.md)"
    backlog_before="$(cat BACKLOG.md)"
    # A --prefix that names no prefix is refused rather than read as "no filter". Read as no
    # filter it would archive every prefix's tickets, which is the opposite of what it asked for.
    for empty in "" ","; do
      if "$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --prefix "$empty" --archive v0.1.0 v0.0.0 HEAD >empty.out 2>&1; then
        echo "self-test: --prefix '$empty' must be refused, not read as no filter:"; cat empty.out; exit 1
      fi
      grep -q 'at least one prefix' empty.out || { echo "self-test: the refusal should say what is wrong:"; cat empty.out; exit 1; }
      [ "$changelog_before" = "$(cat CHANGELOG.md)" ] || { echo "self-test: a refused --prefix rewrote the changelog"; exit 1; }
      [ "$backlog_before" = "$(cat BACKLOG.md)" ] || { echo "self-test: a refused --prefix rewrote the backlog"; exit 1; }
    done
    if ! "$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --prefix ZZ --archive v0.1.0 v0.0.0 HEAD >zz.out 2>&1; then
      echo "self-test: a prefix matching nothing must still exit 0:"; cat zz.out; exit 1
    fi
    grep -q 'nothing archived' zz.out || { echo "self-test: a prefix matching nothing said nothing:"; cat zz.out; exit 1; }
    [ "$changelog_before" = "$(cat CHANGELOG.md)" ] || { echo "self-test: a prefix matching nothing rewrote the changelog"; exit 1; }
    [ "$backlog_before" = "$(cat BACKLOG.md)" ] || { echo "self-test: a prefix matching nothing rewrote the backlog"; exit 1; }
    # Put the fixture back the way the rest of the test expects it, then prove the flag-less run
    # still behaves exactly as it did before.
    rm -f CHANGELOG.md zz.out empty.out
    git checkout -- BACKLOG.md
    "$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --archive v0.1.0 v0.0.0 HEAD >/dev/null
    grep -q '^# Changelog' CHANGELOG.md || { echo "self-test: no changelog header"; cat CHANGELOG.md; exit 1; }
    grep -q '^## v0.1.0 — [0-9-]* (v0.0.0..HEAD)$' CHANGELOG.md || { echo "self-test: no tag heading:"; cat CHANGELOG.md; exit 1; }
    grep -q '^#### AA-01 First thing — [0-9-]* · [0-9a-f]*$' CHANGELOG.md || { echo "self-test: AA-01 block not archived:"; cat CHANGELOG.md; exit 1; }
    grep -q '^Why the first thing matters.$' CHANGELOG.md || { echo "self-test: AA-01 body not archived"; exit 1; }
    grep -q '^#### BB-01 Other thing' CHANGELOG.md || { echo "self-test: BB-01 block not archived"; exit 1; }
    grep -q '^### AA-01 ' BACKLOG.md && { echo "self-test: AA-01 still in the backlog:"; cat BACKLOG.md; exit 1; }
    grep -q '^### AA-02 Second thing — `todo` — Blocked by AA-01$' BACKLOG.md || { echo "self-test: AA-02 lost from the backlog:"; cat BACKLOG.md; exit 1; }
    grep -q '^## Beta$' BACKLOG.md || { echo "self-test: the Beta section heading must stay"; exit 1; }
    # A second release: only what shipped since the last tag, prepended above it.
    git tag v0.1.0
    git commit -q --allow-empty -m "AA-02: second thing"
    "$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --archive v0.2.0 v0.1.0 HEAD >/dev/null
    [ "$(grep -n '^## v0' CHANGELOG.md | head -1)" != "" ] || exit 1
    first_tag="$(grep '^## v0' CHANGELOG.md | head -1)"
    case "$first_tag" in "## v0.2.0"*) ;; *) echo "self-test: newest release must come first: $first_tag"; exit 1 ;; esac
    grep -q '^#### AA-02 Second thing' CHANGELOG.md || { echo "self-test: AA-02 not archived in the second release"; exit 1; }
    [ "$(grep -c '^#### AA-01 ' CHANGELOG.md)" = 1 ] || { echo "self-test: AA-01 archived twice"; exit 1; }
    grep -q '^### ' BACKLOG.md && { echo "self-test: the backlog should hold no tickets now:"; cat BACKLOG.md; exit 1; }
    # Archiving again with nothing new changes nothing.
    before="$(cat CHANGELOG.md)"
    "$ROOT/scripts/release-notes.sh" --backlog BACKLOG.md --changelog CHANGELOG.md --archive v0.2.0 v0.1.0 HEAD >/dev/null
    [ "$before" = "$(cat CHANGELOG.md)" ] || { echo "self-test: a repeated archive must not change the changelog"; exit 1; }
  )
  echo "release-notes self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  *)
    [ "${#REFS[@]}" -ge 1 ] || { echo "usage: scripts/release-notes.sh [--archive <tag>] <from-ref> [<to-ref>]" >&2; exit 2; }
    FROM="${REFS[0]}"
    TO="${REFS[1]:-$("$ROOT/scripts/loop-config.sh" default_branch)}"
    # The backlog's own repository answers; a copy outside any repository (a scratch archive)
    # is judged by this repository's history.
    REPO="$(dirname "$BACKLOG")"
    git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1 || REPO="$ROOT"
    cd "$REPO"
    if [ -n "$ARCHIVE" ]; then
      shipped "$FROM" "$TO" | render archive "$ARCHIVE" "$FROM" "$TO" "$BACKLOG" "$CHANGELOG"
    else
      shipped "$FROM" "$TO" | render notes "" "$FROM" "$TO" "$BACKLOG" "$CHANGELOG"
    fi
    ;;
esac
