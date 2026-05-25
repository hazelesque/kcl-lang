//! User-observable smoke test for the runnable example binary.
//!
//! This is the regression test for the silent-spans bug (kcl-lang
//! commit `108292b6`) at the *actual user-observable* layer: spawn
//! the example binary with `KCL_LOG=info`, capture stderr, assert
//! the Phase 6b boundary span names appear in the formatted output.
//!
//! Reviewer 1's structural learning from the bug: "regression tests
//! need to verify the actual user-observable property, not the
//! nearest convenient internal property". The previous
//! `evaluation_emits_expected_phase_spans` test in `kcl-embed` checks
//! that spans dispatch to a Layer; the `evaluation_emits_spans_visible_in_fmt_output`
//! test checks they render to a fmt writer with the right config. But
//! neither tests the *binary's specific subscriber config*. If a
//! future contributor changes `init_tracing_subscriber` (in
//! `crates/cmd/src/lib.rs` for the real CLI, or `src/main.rs` here
//! for the example) and removes `with_span_events(...)` or shifts
//! the default filter past `info`, those library-layer tests still
//! pass — only this subprocess test would catch it.
//!
//! Cost: the example binary has to be built before this test runs.
//! `cargo test -p embed-with-codegen-example` does this automatically
//! (cargo builds bin targets that tests depend on via env! at
//! compile time); the `CARGO_BIN_EXE_<name>` env var points at the
//! freshly-built binary.

use std::process::Command;

/// Helper for the CARGO_BIN_EXE_ lookup. cargo populates this env
/// var at compile time for any binary in the crate, so the test
/// always picks up the just-built example.
fn example_binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_embed-with-codegen-example")
}

/// Running the example with the default filter (no `KCL_LOG` set)
/// produces only the example's own stdout output — no tracing noise
/// on stderr. Locks down the "casual `cargo run` stays quiet"
/// contract from the example's subscriber-init docstring.
#[test]
fn default_invocation_is_quiet_on_stderr() {
    let output = Command::new(example_binary_path())
        .env_remove("KCL_LOG")
        .env_remove("RUST_LOG")
        .output()
        .expect("failed to run example binary");

    assert!(
        output.status.success(),
        "example exited non-zero: status={:?} stderr=\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The example uses eprintln!("[kcl] {msg}") for its own
    // log_messages forwarding — those lines are explicitly stderr by
    // design (the library never writes to stderr; this is the
    // consumer doing it). Filter those out before asserting "quiet".
    let tracing_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.starts_with("[kcl]"))
        .collect();
    assert!(
        tracing_lines.is_empty(),
        "expected no tracing output without KCL_LOG; got:\n{}",
        tracing_lines.join("\n"),
    );
}

/// Running with `KCL_LOG=info` produces the Phase 6b boundary spans
/// on stderr.
///
/// This is the regression test for the silent-spans bug: if the
/// example's `init_tracing_subscriber` (or the analogous code in
/// `kcl_cmd`) loses `with_span_events(ENTER | EXIT)`, this assertion
/// fails because the buffer would only contain event output, not
/// span lifecycle markers — and the boundary spans contain no
/// `info!`/`debug!` events inside them so the output would be empty.
#[test]
fn kcl_log_info_emits_boundary_spans_to_stderr() {
    let output = Command::new(example_binary_path())
        .env("KCL_LOG", "info")
        .env_remove("RUST_LOG")
        .output()
        .expect("failed to run example binary");

    assert!(
        output.status.success(),
        "example exited non-zero: status={:?} stderr=\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in &[
        "kcl_embedded_evaluate",
        "kcl_parse",
        "kcl_resolve",
        "kcl_evaluate",
    ] {
        assert!(
            stderr.contains(required),
            "expected stderr to mention span {required:?}; got:\n{stderr}"
        );
    }
    // ENTER/EXIT markers prove `with_span_events(...)` is wired —
    // without that subscriber config the spans dispatch but render
    // as nothing.
    assert!(
        stderr.contains("enter") && stderr.contains("exit"),
        "expected both 'enter' and 'exit' markers in stderr; got:\n{stderr}"
    );
}
