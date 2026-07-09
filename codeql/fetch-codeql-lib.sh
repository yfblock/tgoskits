#!/usr/bin/env bash
# Fetch the upstream CodeQL Rust query library (codeql/rust-all + shared deps)
# from github.com/github/codeql, so the mmio-audit queries can compile and
# render results with file:line locations.
#
# The locally installed CodeQL CLI (/opt/codeql) ships only the Rust extractor
# + raw dbscheme, NOT the codeql/rust-all query library, and the library
# cannot be downloaded from the GitHub Container Registry in this environment
# (HTTP 403). Cloning the source repo and pointing --search-path at it works.
#
# Re-run is a no-op if the clone already exists.
set -euo pipefail

LIB_DIR="$(cd "$(dirname "$0")" && pwd)/codeql-lib"
if [ -d "$LIB_DIR/.git" ]; then
  echo "codeql-lib already present at $LIB_DIR"
  exit 0
fi

echo "Sparse-cloning github/codeql (rust + shared library packs) into $LIB_DIR ..."
git clone --depth 1 --filter=blob:none --sparse https://github.com/github/codeql.git "$LIB_DIR"
cd "$LIB_DIR"
git sparse-checkout set rust/ql/lib rust/ql/src shared

echo "Done. Use --search-path=$LIB_DIR with codeql database analyze."
