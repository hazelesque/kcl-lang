//! Integration tests for the Phase 3 embedding API. Exercises the
//! full register_module -> build -> evaluate flow end-to-end through
//! the loader-side virtual_packages hook landed in step 2.

use kcl_embed::{Embedded, EvaluateArgs, EvaluationError};

/// Main source imports a registered module; the embedded VM resolves
/// the import in-memory, the schema's default flows through, and the
/// typed ValueRef tree contains the expected structure.
#[test]
fn main_imports_registered_module_and_uses_default() {
    let mut embedded = Embedded::new();
    embedded
        .register_module(
            "tilley",
            concat!(
                "schema Vm:\n",
                "    name: str\n",
                "    memory_mb: int = 1024\n",
            ),
        )
        .expect("register_module");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import tilley\n",
                "vm = tilley.Vm {name = \"alpha\"}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate should succeed");

    assert!(outcome.value.is_dict());
    let vm = outcome.value.dict_get_value("vm").expect("vm");
    assert!(vm.is_schema(), "vm should be a schema instance");
    assert_eq!(vm.as_schema().name, "Vm");
    assert_eq!(vm.dict_get_value("name").unwrap().as_str(), "alpha");
    assert_eq!(
        vm.dict_get_value("memory_mb").unwrap().as_int(),
        1024,
        "schema default did not flow through"
    );
}

/// Registered modules can `import` each other. Main imports A;
/// A `import`s B; both pkgs appear in the program and the schema from
/// B is reachable through A.
#[test]
fn registered_modules_can_transitively_import_each_other() {
    let mut embedded = Embedded::new();
    embedded
        .register_module(
            "tilley_vm",
            concat!("schema VmSpec:\n", "    cores: int = 2\n"),
        )
        .expect("register tilley_vm");
    embedded
        .register_module(
            "tilley",
            concat!(
                "import tilley_vm\n",
                "\n",
                "schema Vm:\n",
                "    name: str\n",
                "    spec: tilley_vm.VmSpec = tilley_vm.VmSpec {}\n",
            ),
        )
        .expect("register tilley");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import tilley\n",
                "vm = tilley.Vm {name = \"alpha\"}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let vm = outcome.value.dict_get_value("vm").expect("vm");
    let spec = vm.dict_get_value("spec").expect("vm.spec");
    assert!(spec.is_schema(), "vm.spec should be a schema instance");
    assert_eq!(spec.as_schema().name, "VmSpec");
    assert_eq!(spec.dict_get_value("cores").unwrap().as_int(), 2);
}

/// Multi-diagnostic resolve-stage error: importing two distinct
/// unregistered modules surfaces both `pkgpath ... not found`
/// diagnostics in a single EvaluationError::Resolve. Locks down the
/// "diagnostics carry through as a Vec, not just the first one"
/// property of the new error shape.
#[test]
fn multi_diagnostic_resolve_error_carries_all_failures() {
    let ready = Embedded::new().build();

    let err = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import unregistered_pkg_a\n",
                "import unregistered_pkg_b\n",
                "a = 1\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("should fail with resolve diagnostics");

    let diags = match err {
        EvaluationError::Resolve(d) => d,
        EvaluationError::Parse(d) => d,
        other => panic!("expected Resolve or Parse error, got: {other:?}"),
    };
    assert!(
        diags.len() >= 2,
        "expected >=2 diagnostics (one per unregistered import); got {}: {:#?}",
        diags.len(),
        diags
    );
    let messages: Vec<&str> = diags
        .iter()
        .flat_map(|d| d.messages.iter().map(|m| m.message.as_str()))
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unregistered_pkg_a") && m.contains("not found")),
        "expected diagnostic mentioning unregistered_pkg_a; got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unregistered_pkg_b") && m.contains("not found")),
        "expected diagnostic mentioning unregistered_pkg_b; got: {messages:?}"
    );
}

/// Runtime check-block violations surface as EvaluationError::Evaluate
/// with a non-empty diagnostic vec. Confirms the runtime-error path
/// reaches the structured-error surface (rather than e.g. ending up
/// as a panic in Internal).
#[test]
fn runtime_check_block_violation_surfaces_as_evaluate_error() {
    let mut embedded = Embedded::new();
    embedded
        .register_module(
            "tilley",
            concat!(
                "schema Vm:\n",
                "    memory_mb: int\n",
                "    check:\n",
                "        memory_mb >= 1024, \"memory_mb must be at least 1024\"\n",
            ),
        )
        .expect("register");
    let ready = embedded.build();

    let err = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import tilley\n",
                "vm = tilley.Vm {memory_mb = 16}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("check block should fire");

    match err {
        EvaluationError::Evaluate(diags) => {
            assert!(!diags.is_empty(), "expected at least one Evaluate diagnostic");
            // Phase 6a will refine the conversion; for now we just
            // assert the message mentions the check predicate.
            let messages: Vec<&str> = diags
                .iter()
                .flat_map(|d| d.messages.iter().map(|m| m.message.as_str()))
                .collect();
            assert!(
                messages
                    .iter()
                    .any(|m| m.contains("memory_mb must be at least 1024")),
                "expected check-block message in diagnostics; got: {messages:?}"
            );
        }
        other => panic!("expected Evaluate error, got: {other:?}"),
    }
}

/// `print()` output is collected into `EvaluateOutcome::log_messages`;
/// the library does not write to stderr. Locks down the no-rude-stderr
/// contract from D3.
#[test]
fn print_output_collected_into_log_messages() {
    let ready = Embedded::new().build();
    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "print(\"first\")\n",
                "print(\"second\")\n",
                "result = 42\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    assert_eq!(outcome.value.dict_get_value("result").unwrap().as_int(), 42);
    assert!(
        outcome.log_messages.iter().any(|m| m.contains("first")),
        "missing 'first' in log_messages: {:?}",
        outcome.log_messages
    );
    assert!(
        outcome.log_messages.iter().any(|m| m.contains("second")),
        "missing 'second' in log_messages: {:?}",
        outcome.log_messages
    );
}

/// EvaluateArgs::external_args (the `-D key=value` surface) propagates
/// to the runtime's `option()` builtin.
#[test]
fn external_args_reach_option_builtin() {
    let ready = Embedded::new().build();
    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: "host = option(\"hostname\")".to_string(),
            external_args: vec![("hostname".to_string(), "\"alpha.example\"".to_string())],
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    assert_eq!(
        outcome.value.dict_get_value("host").unwrap().as_str(),
        "alpha.example",
        "option() didn't surface external_args value"
    );
}

/// Phase 6b: install a process-wide tracing recorder once (the test
/// binary's first test through this helper wins; subsequent tests
/// share the same recorder). Then trigger an Embedded::evaluate and
/// assert the Phase 6b boundary spans appear in the recorder's
/// accumulated log.
///
/// This recorder pattern handles the parallel-test reality cleanly:
/// every test that calls evaluate() will dispatch spans into the
/// shared recorder, so the assertion holds regardless of which other
/// tests are running concurrently. The recorder accumulates spans
/// from all tests in this binary; we only assert *presence*, not
/// exclusivity. Using `with_default` instead would fight tracing's
/// callsite interest cache under parallel execution (a known footgun
/// — `with_default` is thread-local, but tracing's per-callsite
/// Interest can poison once seen with no subscriber attached).
///
/// If a future refactor removes the `kcl_parse` / `kcl_resolve` /
/// `kcl_evaluate` spans or renames the top-level
/// `kcl_embedded_evaluate` span, this test fails.
#[test]
fn evaluation_emits_expected_phase_spans() {
    use std::sync::{Arc, Mutex, OnceLock};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    #[derive(Clone, Default)]
    struct SpanRecorder {
        observed: Arc<Mutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for SpanRecorder
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _id: &tracing::span::Id,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if let Ok(mut guard) = self.observed.lock() {
                guard.push(attrs.metadata().name().to_string());
            }
        }
    }

    static SHARED_RECORDER: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();

    let observed = SHARED_RECORDER
        .get_or_init(|| {
            let recorder = SpanRecorder::default();
            let observed = recorder.observed.clone();
            // try_init returns Err if a global subscriber already
            // exists; either way, our OnceLock-stored Arc is the one
            // we'll read from when the test asserts. The dispatch
            // result here only matters if we won the install race.
            let _ = tracing_subscriber::registry()
                .with(recorder)
                .try_init();
            observed
        })
        .clone();

    let observed_before = observed.lock().unwrap().len();

    let mut embedded = Embedded::new();
    embedded
        .register_module(
            "tilley",
            "schema Vm:\n    name: str\n    memory_mb: int = 1024\n",
        )
        .expect("register");
    let ready = embedded.build();
    let _ = ready
        .evaluate(EvaluateArgs {
            main_source: "import tilley\nvm = tilley.Vm {name = \"alpha\"}\n"
                .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let snapshot = observed.lock().unwrap().clone();
    // Either look at just our window OR the full accumulated log;
    // either way the four span names must be present. Using the full
    // log is more tolerant of concurrent test span ordering.
    let _ = observed_before; // window unused; full snapshot is sufficient
    for required in &[
        "kcl_embedded_evaluate",
        "kcl_parse",
        "kcl_resolve",
        "kcl_evaluate",
    ] {
        assert!(
            snapshot.iter().any(|s| s == required),
            "expected span {required:?} not observed; got: {snapshot:?}"
        );
    }
}

/// Phase 6a step 4 eliminated the PanicInfo → JSON → string → parse →
/// PanicInfo → Diagnostic → string round-trip in favour of a direct
/// PanicInfo → Diagnostic → string conversion in `emit_panic_info_to_string`.
///
/// This test locks down the *absence* of the JSON intermediate: a
/// runtime-triggered panic (here, a check-block violation) surfaces as
/// an `EvaluationError::Evaluate` whose diagnostic message contains the
/// user-facing predicate text but does NOT contain the raw JSON field
/// names that would appear if `PanicInfo::to_json_string()` were in the
/// pipeline (`kcl_pkgpath`, `rust_file`, `backtrace`).
///
/// Skipped if the consumer has explicitly opted into the JSON debug
/// channel via `KCL_DEBUG_ERROR=1` — that env var is the documented
/// escape hatch and reverting the diagnostic to JSON is its
/// contracted behaviour.
#[test]
fn runtime_panic_surfaces_as_diagnostic_not_json() {
    if std::env::var("KCL_DEBUG_ERROR").is_ok() {
        eprintln!(
            "skipping runtime_panic_surfaces_as_diagnostic_not_json: \
             KCL_DEBUG_ERROR is set, which forces the JSON debug channel"
        );
        return;
    }

    let mut embedded = Embedded::new();
    embedded
        .register_module(
            "tilley",
            concat!(
                "schema Vm:\n",
                "    memory_mb: int\n",
                "    check:\n",
                "        memory_mb >= 1024, \"memory_mb must be at least 1024\"\n",
            ),
        )
        .expect("register");
    let ready = embedded.build();

    let err = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import tilley\n",
                "vm = tilley.Vm {memory_mb = 16}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("check block should fire");

    let diags = match err {
        EvaluationError::Evaluate(diags) => diags,
        other => panic!("expected Evaluate error, got: {other:?}"),
    };

    let all_messages: String = diags
        .iter()
        .flat_map(|d| d.messages.iter().map(|m| m.message.as_str()))
        .collect::<Vec<_>>()
        .join("\n");

    // Positive: the user-facing predicate text reaches the consumer.
    assert!(
        all_messages.contains("memory_mb must be at least 1024"),
        "expected check predicate text in diagnostics; got: {all_messages:?}"
    );

    // Negative: the raw PanicInfo JSON field names must NOT appear.
    // Their presence would mean the panic info was serialised to JSON
    // somewhere in the pipeline, which is exactly what step 4 removed.
    for json_marker in &["kcl_pkgpath", "rust_file", "\"backtrace\""] {
        assert!(
            !all_messages.contains(json_marker),
            "diagnostic message contains JSON-shape marker {json_marker:?} — \
             the PanicInfo JSON round-trip may have crept back in: {all_messages:?}"
        );
    }
}
