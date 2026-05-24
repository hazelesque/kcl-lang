# Logging Conventions

This document records the logging/instrumentation discipline for the
KCL fork, established by Phase 6b of the restructuring plan.

## TL;DR

- Use `tracing` macros for structured logging — never `println!` or
  `eprintln!` for anything other than CLI user-facing output.
- Inside per-evaluation hot loops (per-AST-node, per-field,
  per-attribute, per-symbol), the only macros permitted are `trace!`
  and `debug!`. Both compile out to no-ops in release builds.
- `info!`, `warn!`, and `error!` are reserved for once-per-evaluation
  events (program start, program completion, top-level errors that
  the user sees).
- Spans (`info_span!`, `#[instrument]`) are appropriate for
  evaluation phases (parse → resolve → evaluate) but not for
  hot-path iteration steps.

## Why the hot-loop rule

KCL evaluation is on a real-world performance-sensitive path. Tilley
evaluates ~12 schemas with ~hundreds of attribute lookups per
evaluation; a future intent-based-infrastructure pipeline may
evaluate thousands of attributes per second.

`info!`/`warn!`/`error!` macros are NOT elided in release builds.
Their cost is real: subscriber dispatch, level filtering, span
correlation, and string formatting all run per invocation. Putting
any of those inside a per-AST-node loop costs O(program size) work
per evaluation even when nobody is listening to the logs.

`trace!`/`debug!` macros ARE elided in release builds via the
`tracing/release_max_level_info` feature set on the workspace
dependency in `Cargo.toml`. The macros expand to literally nothing
when the compile-time max level is `info`, so hot-path instrumentation
imposes zero runtime cost on release consumers.

The rule:

> Inside per-evaluation hot loops, you may use `trace!` and `debug!`.
> You may NOT use `info!`, `warn!`, or `error!`.

A "hot loop" here means anything that runs once per AST node, once
per schema attribute, once per symbol, or otherwise scales with
program size. If you're unsure, ask: would this fire 10x, 100x, or
1000x in a typical evaluation? If yes, it's a hot path.

## CI enforcement

`scripts/check-no-hot-path-info.sh` (added in Phase 6b step 5) greps
the hot-path module roots for `info!`/`warn!`/`error!` invocations
and fails the build if any are found:

```sh
! grep -rnE '(info|warn|error)!' \
    crates/evaluator/ \
    crates/runtime/src/value/ \
    crates/runtime/src/api/
```

If a future contributor genuinely needs a once-per-evaluation `info!`
inside one of these modules, the PR explicitly amends the script's
exclusion list with reviewer scrutiny on whether the exclusion is
justified. The default posture is "no" — once-per-evaluation events
belong in the entry-point crates (`kcl-embed`, `kcl-runner`,
`kcl-cmd`), not inside the evaluator's leaf modules.

## Where `println!` IS appropriate

- `crates/cmd/`: user-facing CLI output (e.g. the result of
  `kcl run`). Use `println!` — this is what the user asked for, not
  a logging channel.
- `crates/lib/src/lib.rs`: CLI shim's error output via `eprintln!`
  when the underlying command returns failure.
- `*/tests.rs`: anything goes inside tests.
- `*/build.rs`: build scripts (output is captured by Cargo).
- `*/examples/**/*.rs`: example binaries are user-facing.
- Doctest comments (`/// println!(...)`) inside `///` blocks.

## Subscriber setup

CLI binaries (`kcl`, `kcl-language-server`) install a
`tracing_subscriber::fmt` subscriber gated on the `KCL_LOG` env var:

```rust
let _ = tracing_subscriber::fmt()
    .with_env_filter(
        tracing_subscriber::EnvFilter::try_from_env("KCL_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    )
    .try_init();
```

Library crates (`kcl-embed`, `kcl-runner`, `kcl-api`) do NOT install
a subscriber — that's the consumer's responsibility. If no subscriber
is installed, the macros become no-ops by default.

## Phase 6b audit of existing `println!` sites

Audit performed during Phase 6b step 2 (commit ref recorded in the
Phase 6b step-2 commit body). Of 92 `println!`/`eprintln!`
occurrences across 26 files, zero are in production hot-path code:

- 49 in `*/tests.rs`, `tests/*.rs`, `benches/*.rs` — tests
- 9 in `*/build.rs` — build scripts
- 16 in `crates/cmd/`, `crates/lib/`, `crates/tools/{format,fix,LSP,vet}/`
  — user-facing CLI output and developer tools (not VM hot path)
- 14 in `crates/embed/examples/`, `crates/rust-codegen/examples/` —
  example binaries
- 2 in `crates/error/src/error.rs` — doctest comments inside `///`
- 2 in `crates/sema/src/advanced_resolver/mod.rs` is a
  `#[allow(unused)]` debug helper (`print_symbols_info`) that dumps
  symbol info in a format suitable for copy-pasting into test
  fixtures; intentional human-readable output, not instrumentation

The hot-path crates (`kcl-evaluator`, `kcl-runtime`,
`kcl-runner` (production paths), `kcl-loader`, `kcl-parser`,
`kcl-sema` production paths) carried zero `println!` calls before
Phase 6b began. The discipline was already in place; Phase 6b's
contribution is the affirmative `tracing` instrumentation in step 3
plus the CI grep in step 5 to keep it that way.
