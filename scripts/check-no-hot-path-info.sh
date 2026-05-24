#!/bin/sh
#
# Phase 6b step 5: enforce the no-info!-in-hot-loops rule.
#
# Inside per-evaluation hot loops (per-AST-node, per-field,
# per-attribute, per-symbol), only `trace!` and `debug!` are
# permitted — both compile out to no-ops in release builds via the
# `tracing/release_max_level_info` feature set on the workspace dep.
# `info!`/`warn!`/`error!` are reserved for once-per-evaluation
# events at the entry-point crates (kcl-embed, kcl-runner, kcl-cmd),
# not the leaf modules.
#
# This script greps the hot-path module roots and fails if it finds
# any `info!`/`warn!`/`error!` macro calls. If a future contributor
# has a justified once-per-evaluation site inside one of these
# modules, amend this script's `--exclude` list with reviewer
# scrutiny on whether the exclusion is warranted.
#
# Run locally before pushing:
#
#   ./scripts/check-no-hot-path-info.sh
#
# CI wires this into the standard linux job (see .github/workflows/).
#
# See docs/dev_guide/logging-conventions.md for the full rule and
# rationale.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

HOT_PATH_DIRS="
  crates/evaluator/src
  crates/runtime/src/value
  crates/runtime/src/api
"

# Match `info!`, `warn!`, `error!` macro invocations. The trailing `!`
# distinguishes a macro from a function or type named `info`/`warn`/
# `error` (e.g. `log::Level::Error` would not match).
PATTERN='\b(info|warn|error)!'

# Run the search. We use ripgrep when available (fast, .gitignore-aware),
# falling back to grep -r otherwise.
SEARCH_DIRS=""
for d in $HOT_PATH_DIRS; do
    full="$REPO_ROOT/$d"
    if [ -d "$full" ]; then
        SEARCH_DIRS="$SEARCH_DIRS $full"
    fi
done

if [ -z "$SEARCH_DIRS" ]; then
    echo "no hot-path dirs found; expected at least one of:"
    echo "$HOT_PATH_DIRS"
    exit 2
fi

if command -v rg >/dev/null 2>&1; then
    # shellcheck disable=SC2086
    matches=$(rg --type rust --line-number --no-heading "$PATTERN" $SEARCH_DIRS || true)
else
    # shellcheck disable=SC2086
    matches=$(grep -rnE --include='*.rs' "$PATTERN" $SEARCH_DIRS || true)
fi

if [ -n "$matches" ]; then
    cat >&2 <<EOF
ERROR: hot-path info!/warn!/error! invocations found.

These macros do NOT compile out in release builds. Putting them inside
per-evaluation hot loops costs O(program size) work per evaluation
even when nobody is reading the logs.

Use trace!/debug! instead (both elided in release via tracing's
release_max_level_info feature on the workspace dep). Reserve info!+
for once-per-evaluation events at the entry-point crates.

See docs/dev_guide/logging-conventions.md for the full rule.

Offending lines:
$matches
EOF
    exit 1
fi

echo "ok: no hot-path info!/warn!/error! invocations"
