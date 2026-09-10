#!/usr/bin/env bash
# The loop's settings (HK-14). Everything project-specific that scripts/backlog-status.sh,
# scripts/open-ticket-pr.sh, and scripts/release-notes.sh need comes from `.loop.toml` at the
# repository root, a single flat `[loop]` table, through this reader; the scripts themselves
# name no branch, path, or command of this project.
#
#   scripts/loop-config.sh <key>        print the value (arrays: one item per line; booleans:
#                                       true/false); a missing file or key gives the default
#   scripts/loop-config.sh --all        every key with its effective value
#   scripts/loop-config.sh --self-test  a fixture proves: missing file, missing key, each key set
#
# Keys and defaults:
#   default_branch    "main"                 the branch pull requests target and done is judged on
#   backlog           "BACKLOG.md"           the ticket file, relative to the root
#   check             "scripts/check.sh"     the full check, run before a commit
#   check_fast        (the value of check)   the check to run while iterating
#   review_paths      []                     globs (shell patterns, or path prefixes) whose change
#                                            turns auto-merge off and labels the PR needs-review
#   trailer_required  true                   commits must carry a Co-Authored-By trailer naming
#                                            the agent that did the work
#   kit               ""                     where the loop kit lives: a directory (relative to
#                                            the root) or a git URL; scripts/loop-kit-sync.sh
#                                            copies its scripts and prompts in and diffs them
#   kit_ref           ""                     the kit's tag or branch when kit is a URL
#   code_paths        []                     globs whose change needs a proof (the proof gate,
#                                            scripts/proof-gate.sh); empty: gate off
#   proof_paths       []                     globs that count as proof when changed
#   proof_pattern     ""                     a regex; an added line matching it counts as proof
#
# LOOP_ROOT overrides the root (the fixture repos of the self-tests); LOOP_CONFIG names another
# file outright. TOML is only the config format: it says nothing about the project's language.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
FILE="${LOOP_CONFIG:-$ROOT/.loop.toml}"
MODE=get
KEY="${1:-}"
case "$KEY" in
  --self-test) MODE=selftest ;;
  --all) MODE=all ;;
  "") echo "usage: scripts/loop-config.sh <key> | --all | --self-test" >&2; exit 2 ;;
esac

# Reads the [loop] table of a TOML file (strings, booleans, one-line string arrays; # comments;
# the escapes \" and \\ inside strings) and prints the requested key, or every key, with the
# defaults above filled in.
read_config() {
  local file="$1" mode="$2" key="$3"
  perl -e '
    use strict; use warnings;
    my ($file, $mode, $key) = @ARGV;
    my @order = qw(default_branch backlog check check_fast review_paths trailer_required kit kit_ref
                   code_paths proof_paths proof_pattern);
    my %default = (default_branch => "main", backlog => "BACKLOG.md", check => "scripts/check.sh",
                   check_fast => undef, review_paths => [], trailer_required => "true",
                   kit => "", kit_ref => "", code_paths => [], proof_paths => [], proof_pattern => "");
    my %value;
    if (open my $fh, "<", $file) {
      my $table = "";
      while (my $line = <$fh>) {
        chomp $line;
        $line =~ s/^\s+|\s+$//g;
        next if $line eq "" || $line =~ /^#/;
        if ($line =~ /^\[(.+)\]$/) { $table = $1; next; }
        next unless $table eq "loop";
        my ($k, $raw) = $line =~ /^([A-Za-z0-9_-]+)\s*=\s*(.*)$/ or die "cannot parse $file: $line\n";
        unless (exists $default{$k}) { warn "$file: unknown key $k in [loop]\n"; next; }
        if ($raw =~ /^\[(.*)\]\s*(?:#.*)?$/) {
          my @items = $1 =~ /"((?:[^"\\]|\\.)*)"/g;
          s/\\(["\\])/$1/g for @items;
          $value{$k} = \@items;
        } elsif ($raw =~ /^"((?:[^"\\]|\\.)*)"\s*(?:#.*)?$/) {
          (my $s = $1) =~ s/\\(["\\])/$1/g; $value{$k} = $s;
        } elsif ($raw =~ /^(true|false)\s*(?:#.*)?$/) {
          $value{$k} = $1;
        } else { die "cannot parse $file: $line (strings are double-quoted; arrays are one line)\n"; }
      }
      close $fh;
    }
    # check_fast falls back to check, whatever check is.
    my $effective = sub {
      my $k = shift;
      my $v = exists $value{$k} ? $value{$k} : $default{$k};
      $v = exists $value{check} ? $value{check} : $default{check} if $k eq "check_fast" && !defined $v;
      return $v;
    };
    my $show = sub { my $v = shift; return ref $v ? join("\n", @$v) : $v; };
    if ($mode eq "all") {
      for my $k (@order) { my $v = $effective->($k); print "$k = ", (ref $v ? "[" . join(", ", map { "\"$_\"" } @$v) . "]" : (($v =~ /^(true|false)$/) ? $v : "\"$v\"")), "\n"; }
      exit 0;
    }
    die "unknown key $key (one of: @order)\n" unless exists $default{$key};
    my $v = $effective->($key);
    print $show->($v), (ref $v && !@$v ? "" : "\n") if defined $v;
  ' "$file" "$mode" "$key"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/loop-config.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR"
  local me="$SCRIPT_ROOT/scripts/loop-config.sh"
  # A missing file: every key is its default.
  local got
  got="$(LOOP_ROOT="$dir" "$me" default_branch)"; [ "$got" = "main" ] || { echo "self-test: default_branch should default to main, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" backlog)"; [ "$got" = "BACKLOG.md" ] || { echo "self-test: backlog should default to BACKLOG.md, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" check)"; [ "$got" = "scripts/check.sh" ] || { echo "self-test: check default wrong: '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" check_fast)"; [ "$got" = "scripts/check.sh" ] || { echo "self-test: check_fast should fall back to check, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" review_paths)"; [ -z "$got" ] || { echo "self-test: review_paths should default to nothing, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" trailer_required)"; [ "$got" = "true" ] || { echo "self-test: trailer_required should default to true, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" kit)"; [ -z "$got" ] || { echo "self-test: kit should default to nothing, got '$got'"; exit 1; }
  # A file with some keys: the set ones win, the missing ones keep their defaults, and
  # check_fast follows a changed check.
  cat > "$dir/.loop.toml" <<'EOF'
# fixture
[loop]
default_branch = "trunk"   # the branch
check = "make check"
review_paths = ["fixtures/expected/", "docs/*.md"]
trailer_required = false

[other]
default_branch = "ignored"
EOF
  got="$(LOOP_ROOT="$dir" "$me" default_branch)"; [ "$got" = "trunk" ] || { echo "self-test: default_branch should be trunk, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" backlog)"; [ "$got" = "BACKLOG.md" ] || { echo "self-test: a missing key keeps its default, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" check)"; [ "$got" = "make check" ] || { echo "self-test: check should be 'make check', got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" check_fast)"; [ "$got" = "make check" ] || { echo "self-test: check_fast should follow check, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" review_paths)"; [ "$got" = $'fixtures/expected/\ndocs/*.md' ] || { echo "self-test: review_paths should list two globs, got '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" trailer_required)"; [ "$got" = "false" ] || { echo "self-test: trailer_required should be false, got '$got'"; exit 1; }
  # Every key set, including check_fast and an escaped quote; LOOP_CONFIG names the file.
  cat > "$dir/other.toml" <<'EOF'
[loop]
default_branch = "develop"
backlog = "docs/QUEUE.md"
check = "make test"
check_fast = "make lint"
review_paths = ["schema/\"quoted\".json"]
trailer_required = true
kit = "https://example.invalid/loop-kit"
kit_ref = "v1.2.3"
code_paths = ["src/", "lib/"]
proof_paths = ["tests/"]
proof_pattern = "#\\[test\\]|@Test"
EOF
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" backlog)"; [ "$got" = "docs/QUEUE.md" ] || { echo "self-test: backlog should be docs/QUEUE.md, got '$got'"; exit 1; }
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" check_fast)"; [ "$got" = "make lint" ] || { echo "self-test: check_fast should be set, got '$got'"; exit 1; }
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" review_paths)"; [ "$got" = 'schema/"quoted".json' ] || { echo "self-test: escaped quote lost: '$got'"; exit 1; }
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" kit_ref)"; [ "$got" = "v1.2.3" ] || { echo "self-test: kit_ref should be v1.2.3, got '$got'"; exit 1; }
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" code_paths)"; [ "$got" = $'src/\nlib/' ] || { echo "self-test: code_paths should list two globs, got '$got'"; exit 1; }
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" proof_pattern)"; [ "$got" = '#\[test\]|@Test' ] || { echo "self-test: an escaped backslash should come through single: '$got'"; exit 1; }
  got="$(LOOP_ROOT="$dir" "$me" proof_pattern)"; [ -z "$got" ] || { echo "self-test: proof_pattern should default to nothing, got '$got'"; exit 1; }
  got="$(LOOP_CONFIG="$dir/other.toml" "$me" --all | grep -c '=')"; [ "$got" = 11 ] || { echo "self-test: --all should print eleven keys, got $got"; exit 1; }
  # An unknown key is an error; an unknown key in the file is a warning, not a failure.
  if LOOP_ROOT="$dir" "$me" colour >/dev/null 2>&1; then echo "self-test: an unknown key must fail"; exit 1; fi
  printf '[loop]\nfoo = "bar"\n' > "$dir/.loop.toml"
  got="$(LOOP_ROOT="$dir" "$me" default_branch 2>/dev/null)"; [ "$got" = "main" ] || { echo "self-test: unknown file key should be skipped, got '$got'"; exit 1; }
  echo "loop-config self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  all) read_config "$FILE" all "" ;;
  get) read_config "$FILE" get "$KEY" ;;
esac
