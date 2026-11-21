#!/usr/bin/env bash
# Mutation tests of the Rust lines changed since a base ref.
#
# cargo-mutants changes the code one small step at a time and runs the tests
# of the crate after each step. A change that no test notices is listed as
# MISSED and the script ends non-zero. See CONTRIBUTING.md.
#
# Usage: scripts/mutants.sh [base ref] [cargo mutants arguments]
#   scripts/mutants.sh                               # against origin/main
#   scripts/mutants.sh origin/main -p tumult-core    # one crate
#   scripts/mutants.sh origin/main --shard 0/8       # one part of eight
set -euo pipefail

base="${1:-origin/main}"
shift || true
cd "$(git rev-parse --show-toplevel)"

# The work tree against the point where the branch left the base, so lines
# that are not committed yet are tested too.
diff="${CARGO_TARGET_DIR:-target}/mutants-in.diff"
mkdir -p "$(dirname "$diff")"
git diff "$(git merge-base "$base" HEAD)" -- "*.rs" > "$diff"
if [ ! -s "$diff" ]; then
  echo "No Rust line changed since $base: nothing to mutate."
  exit 0
fi
exec cargo mutants --in-diff "$diff" "$@"
