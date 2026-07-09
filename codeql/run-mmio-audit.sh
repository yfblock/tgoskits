#!/usr/bin/env bash
# Build a CodeQL Rust database for the workspace and run the MMIO audit suite.
#
# Usage:
#   codeql/run-mmio-audit.sh [db-path]
#
# Requires:
#   - codeql CLI on PATH
#   - the CodeQL Rust library fetched via codeql/fetch-codeql-lib.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DB="${1:-/tmp/tgoskits-mmio-db}"
LIB="$ROOT/codeql/codeql-lib"

if [ ! -d "$LIB/.git" ]; then
  echo "ERROR: codeql/codeql-lib missing. Run codeql/fetch-codeql-lib.sh first." >&2
  exit 1
fi

# The Rust extractor extracts the WHOLE workspace source regardless of which
# package is built, so building any one leaf crate is enough to audit all of
# the driver/component/virtualization crates that contain ad-hoc MMIO access.
echo "==> Building CodeQL Rust database at $DB"
codeql database create "$DB" \
  --language=rust \
  --source-root="$ROOT" \
  --command="cargo build -p mmio-api" \
  --overwrite

echo "==> Running mmio-audit queries"
codeql database analyze "$DB" \
  "$ROOT/codeql/mmio-audit/AdHocVolatileRegisterAccess.ql" \
  "$ROOT/codeql/mmio-audit/InlineAsmRegisterAccess.ql" \
  --search-path="$LIB" \
  --format=csv \
  --output="$ROOT/codeql/mmio-audit-results.csv"

echo "==> Results: $ROOT/codeql/mmio-audit-results.csv"
wc -l "$ROOT/codeql/mmio-audit-results.csv"
