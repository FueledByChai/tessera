# The loop's own checks, shared by every stack skeleton. Sourced by scripts/check.sh; not a
# script on its own. Expects ROOT and the step function.
loop_checks() {
  step "loop self-tests"
  for s in loop-config backlog-status open-ticket-pr release-notes loop-kit-sync proof-gate coverage-ratchet review-status decisions prompt-check sprint; do
    [ -x "scripts/$s.sh" ] && "scripts/$s.sh" --self-test
  done
  step "loop kit, prompts, and decision records in step"
  if [ -n "$(scripts/loop-config.sh kit)" ]; then scripts/loop-kit-sync.sh --check; else echo "loop kit: none named in .loop.toml (kit); skipped"; fi
  scripts/prompt-check.sh
  scripts/decisions.sh --check
  step "proof gate: code changes bring a proof"
  scripts/proof-gate.sh
}

ratchet() {
  step "coverage ratchet"
  scripts/coverage-ratchet.sh
}
