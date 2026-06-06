//! Integration tests for the Phase 3 embedding API. Exercises the
//! full register_module -> build -> evaluate flow end-to-end through
//! the loader-side virtual_packages hook landed in step 2.

use kcl_embed::{Embedded, EvaluateArgs, EvaluationError, render_diagnostics};

/// Test-runner helper: unwrap an `EvaluationError` into a
/// pretty-rendered string that reads like the kcl CLI / Tilley CLI
/// output. Without this, `evaluate(...).expect("…")` calls bury
/// the diagnostic in a `Resolve([Diagnostic { messages: [Message {
/// range: (..), ... }] ... }])` Debug dump that's a nightmare to
/// scan. Wrapping every test's `evaluate` call with this gives the
/// `failed to compile X:line:col: <human message>` form instead.
///
/// Mirrors `kcl_embed::render_diagnostics` for the Resolve/Evaluate
/// variants and falls back to the plain Display for Internal (which
/// is rare and already prints a useful message).
fn evaluate_or_panic(
    ready: &kcl_embed::EmbeddedReady,
    args: EvaluateArgs,
) -> kcl_embed::EvaluateOutcome {
    match ready.evaluate(args) {
        Ok(outcome) => outcome,
        Err(EvaluationError::Resolve(diags)) | Err(EvaluationError::Evaluate(diags)) => {
            panic!("\n{}\n", render_diagnostics(&diags))
        }
        Err(e) => panic!("evaluate failed: {e}"),
    }
}

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
            main_source: concat!("import tilley\n", "vm = tilley.Vm {name = \"alpha\"}\n",)
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
            main_source: concat!("import tilley\n", "vm = tilley.Vm {name = \"alpha\"}\n",)
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
            main_source: concat!("import tilley\n", "vm = tilley.Vm {memory_mb = 16}\n",)
                .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("check block should fire");

    match err {
        EvaluationError::Evaluate(diags) => {
            assert!(
                !diags.is_empty(),
                "expected at least one Evaluate diagnostic"
            );
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
            main_source: concat!("print(\"first\")\n", "print(\"second\")\n", "result = 42\n",)
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
/// Shared tracing capture state for the Phase 6b span tests. Two
/// independent capture surfaces, both fed by one process-wide
/// subscriber:
///
/// - `span_names` — populated by a custom `Layer::on_new_span` that
///   records the span's metadata name. Tests the dispatch layer:
///   "did the span actually fire?"
/// - `fmt_buffer` — populated by a `tracing_subscriber::fmt::Layer`
///   with `with_span_events(ENTER | EXIT)` writing into an in-memory
///   buffer. Tests what a *user* sees: "did the formatted output
///   actually contain the span name?"
///
/// The split matters because of the silent-spans bug (kcl-lang
/// commit `108292b6`) — the dispatch layer was correct, but the
/// CLI's fmt subscriber lacked `with_span_events(...)` so
/// `KCL_LOG=info kcl run foo.k` was silent on the boundary spans
/// even though the dispatcher was wired. A test that only checked
/// "did the Layer receive the span?" passed; the user-observable
/// "did anything appear on stderr?" property was untested.
struct TracingCaptures {
    span_names: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    fmt_buffer: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

#[derive(Clone)]
struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut guard) = self.0.lock() {
            guard.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuf {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[derive(Clone, Default)]
struct SpanRecorder {
    observed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
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

fn ensure_tracing_init() -> &'static TracingCaptures {
    use std::sync::{Arc, Mutex, OnceLock};
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    static SHARED: OnceLock<TracingCaptures> = OnceLock::new();
    SHARED.get_or_init(|| {
        let span_names = Arc::new(Mutex::new(Vec::new()));
        let fmt_buffer = Arc::new(Mutex::new(Vec::new()));

        let recorder = SpanRecorder {
            observed: span_names.clone(),
        };
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(SharedBuf(fmt_buffer.clone()))
            // Critical: this is what makes the boundary spans visible
            // to the user in formatted output. Without it the spans
            // dispatch but the fmt layer renders nothing.
            .with_span_events(FmtSpan::ENTER | FmtSpan::EXIT)
            // No colours in the captured buffer — assertions need
            // plain text.
            .with_ansi(false);

        // try_init returns Err if a global subscriber already exists;
        // either way our OnceLock-stored captures are the ones tests
        // read from. The dispatch result only matters if we won the
        // install race.
        let _ = tracing_subscriber::registry()
            .with(recorder)
            .with(fmt_layer)
            .try_init();

        TracingCaptures {
            span_names,
            fmt_buffer,
        }
    })
}

fn trigger_evaluate_for_tracing() {
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
            main_source: "import tilley\nvm = tilley.Vm {name = \"alpha\"}\n".to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");
}

#[test]
fn evaluation_emits_expected_phase_spans() {
    let captures = ensure_tracing_init();
    trigger_evaluate_for_tracing();

    let snapshot = captures.span_names.lock().unwrap().clone();
    // The recorder accumulates spans from every test in this binary
    // (process-wide subscriber installed via OnceLock).
    // Asserting *presence* in the full snapshot is sufficient — every
    // evaluate() call exercises the same four spans, so concurrent
    // tests can only add more entries, never remove ours.
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

/// User-observable companion to `evaluation_emits_expected_phase_spans`.
///
/// Where the sibling test asks "did the dispatch layer receive the
/// spans?", this one asks "did the fmt layer render anything a user
/// would see on stderr?". They check different layers of the same
/// pipeline. Both are needed because the dispatch layer can be
/// correct while the fmt layer is silent — see the silent-spans bug
/// caught in kcl-lang commit `108292b6`, where the original Phase 6b
/// span regression test passed but `KCL_LOG=info kcl run foo.k` was
/// silent on the boundary spans because the fmt subscriber config
/// lacked `with_span_events(...)`.
///
/// This test would catch the same class of bug at the test layer
/// rather than waiting for someone to actually run the CLI.
#[test]
fn evaluation_emits_spans_visible_in_fmt_output() {
    let captures = ensure_tracing_init();
    trigger_evaluate_for_tracing();

    let buf = captures.fmt_buffer.lock().unwrap().clone();
    let output = String::from_utf8(buf).expect("fmt output should be UTF-8");

    // Same presence-not-exclusivity argument as the sibling test:
    // every evaluate() call writes the same four spans, so other
    // tests in the binary can only add lines.
    for required in &[
        "kcl_embedded_evaluate",
        "kcl_parse",
        "kcl_resolve",
        "kcl_evaluate",
    ] {
        assert!(
            output.contains(required),
            "expected fmt output to contain {required:?}; got:\n{output}"
        );
    }
    // ENTER/EXIT markers are what `with_span_events` enables — assert
    // their presence to lock down that config too. Without them the
    // spans would still appear via the inheritance chain (each event
    // names its parent span) but with no lifecycle markers, which is
    // exactly the regression mode this test guards against.
    assert!(
        output.contains("enter") || output.contains("exit"),
        "expected enter/exit lifecycle markers in fmt output; got:\n{output}"
    );
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
            main_source: concat!("import tilley\n", "vm = tilley.Vm {memory_mb = 16}\n",)
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

/// Regression test for the lambda-param-shadow bug fixed in
/// `crates/evaluator/src/node.rs` (`walk_config_entries`). Before
/// the fix, a lambda whose body returned a dict literal with field
/// names that matched the lambda's own parameter names would silently
/// turn the field-name LHS into the parameter's *value* (because the
/// evaluator was walking the bare-identifier LHS as an expression
/// when a local var of the same name was in scope). The result was
/// "expect Point, got dict" at the lambda's return-type coercion.
///
/// Tilley's helpers.k tripped this for every helper that uses the
/// `lambda field_name: T -> SomeSchema { { field_name = field_name } }`
/// pattern. The fix: bare-identifier config keys are now always
/// literal field names; explicit subscript syntax
/// (`{[expr] = value}`) is the only path to a dynamic key.
///
/// This test exercises the schema-in-virtual-module case; sibling
/// test `lambda_returning_dict_with_schema_return_type_in_main_source`
/// covers main-source schemas.
#[test]
fn lambda_param_shadow_does_not_break_schema_coercion_in_module() {
    let mut embedded = Embedded::new();
    embedded
        .register_module(
            "m",
            concat!(
                "schema Point:\n",
                "    x: int\n",
                "    y: int\n",
                "\n",
                // x and y here are the *param* names that match the
                // schema field names — the case that used to break.
                "make_point = lambda x: int, y: int -> Point {\n",
                "    {x = x, y = y}\n",
                "}\n",
            ),
        )
        .expect("register");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import m\n",
                "direct = m.Point {x = 1, y = 2}\n",
                "via_lambda = m.make_point(3, 4)\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate should succeed after the param-shadow fix");

    let via_lambda = outcome
        .value
        .dict_get_value("via_lambda")
        .expect("via_lambda");
    assert!(
        via_lambda.is_schema(),
        "lambda body should coerce to schema; was: {}",
        via_lambda.type_str(),
    );
    assert_eq!(via_lambda.as_schema().name, "Point");
    assert_eq!(via_lambda.dict_get_value("x").unwrap().as_int(), 3);
    assert_eq!(via_lambda.dict_get_value("y").unwrap().as_int(), 4);
}

/// Same as `lambda_param_shadow_does_not_break_schema_coercion_in_module`
/// but with the schema and lambda declared in the *main* source —
/// rules out any virtual_packages-specific factor in the fix.
#[test]
fn lambda_param_shadow_does_not_break_schema_coercion_in_main_source() {
    let ready = Embedded::new().build();
    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "schema Point:\n",
                "    x: int\n",
                "    y: int\n",
                "\n",
                "make_point = lambda x: int, y: int -> Point {\n",
                "    {x = x, y = y}\n",
                "}\n",
                "\n",
                "direct = Point {x = 1, y = 2}\n",
                "via_lambda = make_point(3, 4)\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate should succeed after the param-shadow fix");

    let via_lambda = outcome
        .value
        .dict_get_value("via_lambda")
        .expect("via_lambda");
    assert!(via_lambda.is_schema());
    assert_eq!(via_lambda.as_schema().name, "Point");
    assert_eq!(via_lambda.dict_get_value("x").unwrap().as_int(), 3);
    assert_eq!(via_lambda.dict_get_value("y").unwrap().as_int(), 4);
}

/// Renamed-param variant: a lambda whose params *don't* shadow the
/// schema field names always worked. Keeping this test to prove the
/// fix didn't regress the working path either.
#[test]
fn lambda_param_no_shadow_works_under_embed() {
    let ready = Embedded::new().build();
    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "schema Point:\n",
                "    x: int\n",
                "    y: int\n",
                "\n",
                "make_point = lambda xv: int, yv: int -> Point {\n",
                "    {x = xv, y = yv}\n",
                "}\n",
                "\n",
                "p = make_point(7, 9)\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let p = outcome.value.dict_get_value("p").expect("p");
    assert!(p.is_schema());
    assert_eq!(p.as_schema().name, "Point");
    assert_eq!(p.dict_get_value("x").unwrap().as_int(), 7);
    assert_eq!(p.dict_get_value("y").unwrap().as_int(), 9);
}

/// F1.2 — mokkan native type names (`cidr`, `inet`, `macaddr`,
/// `macaddr8`, `IpFamily`) register as built-in named types. A
/// schema declaring fields of these types parses cleanly through
/// sema; values themselves are constructable via string coercion
/// (F1.3) for inet/cidr/MAC types — `IpFamily` waits for the
/// `mokkan.net.V4` constant to land in F1.4.
#[test]
fn mokkan_native_type_names_register_in_schemas() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema NetCfg:\n",
                "    cidr_field?: cidr\n",
                "    inet_field?: inet\n",
                "    mac_field?: macaddr\n",
                "    mac8_field?: macaddr8\n",
                "    family_field?: IpFamily\n",
                "\n",
                "# All optional so we can construct an empty schema.\n",
                "cfg = NetCfg {}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    assert!(cfg.is_schema(), "cfg should be a schema instance");
    assert_eq!(cfg.as_schema().name, "NetCfg");
}

/// F1.3 — string coercion at schema-validation time. A field
/// typed `cidr` accepts a canonical CIDR string and the resulting
/// value carries the `cidr_value` runtime variant (type_str
/// `"cidr"`). Same for `inet` and `macaddr`.
#[test]
fn mokkan_string_coercion_typed_inet_at_validation_time() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema NetCfg:\n",
                "    cidr_field: cidr\n",
                "    inet_field: inet\n",
                "    mac_field: macaddr\n",
                "    mac8_field: macaddr8\n",
                "\n",
                "cfg = NetCfg {\n",
                "    cidr_field = \"10.0.0.0/24\"\n",
                "    inet_field = \"10.0.5.1/24\"\n",
                "    mac_field = \"02:00:00:aa:bb:cc\"\n",
                "    mac8_field = \"02:00:00:00:00:aa:bb:cc\"\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let c = cfg.dict_get_value("cidr_field").unwrap();
    assert_eq!(
        c.type_str(),
        "cidr",
        "cidr_field should now carry a typed cidr_value"
    );
    let i = cfg.dict_get_value("inet_field").unwrap();
    assert_eq!(i.type_str(), "inet");
    let m = cfg.dict_get_value("mac_field").unwrap();
    assert_eq!(m.type_str(), "macaddr");
    let m8 = cfg.dict_get_value("mac8_field").unwrap();
    assert_eq!(m8.type_str(), "macaddr8");
}

/// F1.3 / D8 — `str → cidr` is strict on canonical form. Host bits
/// set surface as a parse failure; the field-validation path
/// keeps the original str value and `check_type` rejects it with
/// "expect cidr, got str". (Better error UX — explicit "host bits
/// were set in cidr value" — is a future polish; the failure is
/// loud-and-loud at this stage.)
#[test]
fn mokkan_string_coercion_strict_cidr_rejects_host_bits_set() {
    let ready = Embedded::new().build();
    let result = ready.evaluate(EvaluateArgs {
        main_source: concat!(
            "schema NetCfg:\n",
            "    cidr_field: cidr\n",
            "\n",
            // 10.0.5.1/24 — host bits are non-zero, parser rejects
            // per D8 strict policy. Operator who wants host bits
            // should declare the field as `inet`.
            "cfg = NetCfg {\n",
            "    cidr_field = \"10.0.5.1/24\"\n",
            "}\n",
        )
        .to_string(),
        ..EvaluateArgs::default()
    });
    let err = result.expect_err("non-canonical CIDR should fail validation");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("cidr") && (msg.contains("str") || msg.contains("expect")),
        "error should mention cidr / str mismatch; got: {msg}"
    );
}

/// F1.4 — PG-shaped algebra functions. Exercises every function
/// in the package once against a known-canonical input to verify
/// PG-documented behaviour. Splits across two main_sources to
/// keep each readable; the test asserts every output rather than
/// the schema shape.
#[test]
fn mokkan_net_algebra_pg_documented_behaviour() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "\n",
                // Source inet: 10.0.5.1/24 — v4, host bits set,
                // exactly the lenient-inet case from F1.3.
                "addr_v4 = \"10.0.5.1/24\"\n",
                "# Single-arg derivations.\n",
                "bcast = net.broadcast(addr_v4)\n",
                "host_ = net.host(addr_v4)\n",
                "masklen_ = net.masklen(addr_v4)\n",
                "netmask_ = net.netmask(addr_v4)\n",
                "host_mask = net.hostmask(addr_v4)\n",
                "network_ = net.network(addr_v4)\n",
                "text_ = net.text(addr_v4)\n",
                "family_ = net.family(addr_v4)\n",
                "# abbrev: /24 not suppressed; /32 case below.\n",
                "abbrev_24 = net.abbrev(addr_v4)\n",
                "abbrev_32 = net.abbrev(\"10.0.5.1/32\")\n",
                "# Binary ops.\n",
                "set_mask = net.set_masklen(addr_v4, 16)\n",
                "merge_ = net.inet_merge(\"10.0.0.0/24\", \"10.0.1.0/24\")\n",
                "# Predicates.\n",
                "contains_strict = net.contains(\"10.0.0.0/8\", \"10.0.5.0/24\")\n",
                "contains_equal = net.contains(\"10.0.0.0/8\", \"10.0.0.0/8\")\n",
                "contains_eq_ = net.contains_eq(\"10.0.0.0/8\", \"10.0.0.0/8\")\n",
                "contained = net.contained_by(\"10.0.5.0/24\", \"10.0.0.0/8\")\n",
                "overlap_yes = net.overlaps(\"10.0.0.0/8\", \"10.0.5.0/24\")\n",
                "overlap_no = net.overlaps(\"10.0.0.0/8\", \"172.16.0.0/12\")\n",
                "same_fam_yes = net.inet_same_family(\"10.0.0.1/24\", \"192.168.0.1/24\")\n",
                "same_fam_no = net.inet_same_family(\"10.0.0.1/24\", \"fd00::1/64\")\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let v = &outcome.value;
    // broadcast(10.0.5.1/24) = 10.0.5.255/24
    assert_eq!(
        format!("{}", v.dict_get_value("bcast").unwrap()),
        "10.0.5.255/24"
    );
    // host: just the address, no masklen.
    assert_eq!(v.dict_get_value("host_").unwrap().as_str(), "10.0.5.1");
    assert_eq!(v.dict_get_value("masklen_").unwrap().as_int(), 24);
    assert_eq!(
        format!("{}", v.dict_get_value("netmask_").unwrap()),
        "255.255.255.0/24"
    );
    assert_eq!(
        format!("{}", v.dict_get_value("host_mask").unwrap()),
        "0.0.0.255/24"
    );
    assert_eq!(
        format!("{}", v.dict_get_value("network_").unwrap()),
        "10.0.5.0/24"
    );
    assert_eq!(v.dict_get_value("text_").unwrap().as_str(), "10.0.5.1/24");
    assert_eq!(format!("{}", v.dict_get_value("family_").unwrap()), "V4");
    assert_eq!(
        v.dict_get_value("abbrev_24").unwrap().as_str(),
        "10.0.5.1/24"
    );
    // /32 suppresses to host form (no masklen).
    assert_eq!(v.dict_get_value("abbrev_32").unwrap().as_str(), "10.0.5.1");
    assert_eq!(
        format!("{}", v.dict_get_value("set_mask").unwrap()),
        "10.0.5.1/16"
    );
    // /24 + /24 adjacent = /23.
    assert_eq!(
        format!("{}", v.dict_get_value("merge_").unwrap()),
        "10.0.0.0/23"
    );
    // Predicates.
    assert!(v.dict_get_value("contains_strict").unwrap().as_bool());
    // strict contains rejects equality.
    assert!(!v.dict_get_value("contains_equal").unwrap().as_bool());
    assert!(v.dict_get_value("contains_eq_").unwrap().as_bool());
    assert!(v.dict_get_value("contained").unwrap().as_bool());
    assert!(v.dict_get_value("overlap_yes").unwrap().as_bool());
    assert!(!v.dict_get_value("overlap_no").unwrap().as_bool());
    assert!(v.dict_get_value("same_fam_yes").unwrap().as_bool());
    assert!(!v.dict_get_value("same_fam_no").unwrap().as_bool());
}

/// F1.4 — `mokkan.net` package registration. `import mokkan.net`
/// binds `net` into scope (KCL leaf-binding); `net.V4` / `net.V6`
/// surface as typed `IpFamily` values; `net.broadcast(inet)`
/// invokes the typed-algebra builtin and returns a typed
/// `inet_value`.
#[test]
fn mokkan_net_package_v4_v6_constants_and_broadcast() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "\n",
                "schema NetCfg:\n",
                "    fam_v4: IpFamily\n",
                "    fam_v6: IpFamily\n",
                "    bcast_v4: inet\n",
                "\n",
                "cfg = NetCfg {\n",
                "    fam_v4 = net.V4\n",
                "    fam_v6 = net.V6\n",
                "    bcast_v4 = net.broadcast(\"10.0.0.1/24\")\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let fam_v4 = cfg.dict_get_value("fam_v4").unwrap();
    assert_eq!(fam_v4.type_str(), "IpFamily");
    assert_eq!(format!("{fam_v4}"), "V4");
    let fam_v6 = cfg.dict_get_value("fam_v6").unwrap();
    assert_eq!(format!("{fam_v6}"), "V6");
    let bcast = cfg.dict_get_value("bcast_v4").unwrap();
    assert_eq!(bcast.type_str(), "inet");
    // 10.0.0.1/24 — broadcast for the /24 is 10.0.0.255/24.
    assert_eq!(format!("{bcast}"), "10.0.0.255/24");
}

/// F1.3 — `str → inet` is lenient. Same "10.0.5.1/24" value that
/// `cidr` rejects above parses cleanly into an `inet`.
#[test]
fn mokkan_string_coercion_lenient_inet_accepts_host_bits_set() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema NetCfg:\n",
                "    inet_field: inet\n",
                "\n",
                "cfg = NetCfg {\n",
                "    inet_field = \"10.0.5.1/24\"\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let i = cfg.dict_get_value("inet_field").unwrap();
    assert_eq!(i.type_str(), "inet");
}

/// F1.5 — `inet + int` / `int + inet` / `inet - int` (offset
/// arithmetic). Masklen preserved from the source inet; the result
/// is a typed `inet_value`.
#[test]
fn mokkan_inet_offset_arithmetic_preserves_masklen() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema NetCfg:\n",
                "    base: inet\n",
                "    plus5: inet\n",
                "    plus5_swapped: inet\n",
                "    minus3: inet\n",
                "\n",
                "base: inet = \"10.0.0.10/24\"\n",
                "\n",
                "cfg = NetCfg {\n",
                "    base = base\n",
                "    plus5 = base + 5\n",
                "    plus5_swapped = 5 + base\n",
                "    minus3 = base - 3\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    assert_eq!(
        format!("{}", cfg.dict_get_value("base").unwrap()),
        "10.0.0.10/24"
    );
    assert_eq!(
        format!("{}", cfg.dict_get_value("plus5").unwrap()),
        "10.0.0.15/24"
    );
    assert_eq!(
        format!("{}", cfg.dict_get_value("plus5_swapped").unwrap()),
        "10.0.0.15/24"
    );
    assert_eq!(
        format!("{}", cfg.dict_get_value("minus3").unwrap()),
        "10.0.0.7/24"
    );
}

/// F1.5 — `inet - inet` (signed distance). v4 - v4 always fits i64;
/// result is a typed int.
#[test]
fn mokkan_inet_subtract_inet_v4_signed_distance() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema NetCfg:\n",
                "    diff_pos: int\n",
                "    diff_neg: int\n",
                "    diff_zero: int\n",
                "\n",
                "a: inet = \"10.0.0.100/24\"\n",
                "b: inet = \"10.0.0.10/24\"\n",
                "\n",
                "cfg = NetCfg {\n",
                "    diff_pos = a - b\n",
                "    diff_neg = b - a\n",
                "    diff_zero = a - a\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    assert_eq!(cfg.dict_get_value("diff_pos").unwrap().as_int(), 90);
    assert_eq!(cfg.dict_get_value("diff_neg").unwrap().as_int(), -90);
    assert_eq!(cfg.dict_get_value("diff_zero").unwrap().as_int(), 0);
}

/// F1.5 — equality on `inet`, `cidr`, and `IpFamily`. The cidr
/// crate's derived PartialEq handles same-value comparisons;
/// distinct values compare unequal.
#[test]
fn mokkan_inet_cidr_ipfamily_equality() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "\n",
                "schema NetCfg:\n",
                "    inet_eq: bool\n",
                "    inet_ne: bool\n",
                "    cidr_eq: bool\n",
                "    cidr_ne: bool\n",
                "    fam_eq: bool\n",
                "    fam_ne: bool\n",
                "    fam_query: bool\n",
                "\n",
                "a: inet = \"10.0.0.1/24\"\n",
                "b: inet = \"10.0.0.1/24\"\n",
                "c: inet = \"10.0.0.2/24\"\n",
                "x: cidr = \"10.0.0.0/24\"\n",
                "y: cidr = \"10.0.0.0/24\"\n",
                "z: cidr = \"10.0.1.0/24\"\n",
                "\n",
                "cfg = NetCfg {\n",
                "    inet_eq = a == b\n",
                "    inet_ne = a != c\n",
                "    cidr_eq = x == y\n",
                "    cidr_ne = x != z\n",
                "    fam_eq = net.V4 == net.V4\n",
                "    fam_ne = net.V4 != net.V6\n",
                "    fam_query = net.family(a) == net.V4\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    for k in [
        "inet_eq",
        "inet_ne",
        "cidr_eq",
        "cidr_ne",
        "fam_eq",
        "fam_ne",
        "fam_query",
    ] {
        assert!(
            cfg.dict_get_value(k).unwrap().as_bool(),
            "expected {k} to be true"
        );
    }
}

/// F1.5 — ordering on `inet` and `cidr`. cidr crate derives Ord;
/// we expose `< <= > >=` directly. Equal addresses with equal mask
/// compare equal in `<=` / `>=`.
#[test]
fn mokkan_inet_cidr_ordering() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema NetCfg:\n",
                "    lt: bool\n",
                "    le_eq: bool\n",
                "    gt: bool\n",
                "    ge_eq: bool\n",
                "    cidr_lt: bool\n",
                "\n",
                "a: inet = \"10.0.0.10/24\"\n",
                "b: inet = \"10.0.0.20/24\"\n",
                "c: inet = \"10.0.0.10/24\"\n",
                "x: cidr = \"10.0.0.0/24\"\n",
                "y: cidr = \"10.0.1.0/24\"\n",
                "\n",
                "cfg = NetCfg {\n",
                "    lt = a < b\n",
                "    le_eq = a <= c\n",
                "    gt = b > a\n",
                "    ge_eq = a >= c\n",
                "    cidr_lt = x < y\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    for k in ["lt", "le_eq", "gt", "ge_eq", "cidr_lt"] {
        assert!(
            cfg.dict_get_value(k).unwrap().as_bool(),
            "expected {k} to be true"
        );
    }
}

/// F1.6 — the `as_cidr` / `as_inet` / `as_macaddr` / `as_macaddr8`
/// / `as_ip_family` `ValueRef` accessors used by codegen-emitted
/// `TryFrom<&ValueRef>` bodies. Each pulls a typed value of the
/// crate type the F1.6 codegen surface promises.
#[test]
fn mokkan_typed_value_accessors_yield_typed_payloads() {
    use std::str::FromStr;

    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "\n",
                "schema NetCfg:\n",
                "    subnet: cidr\n",
                "    addr: inet\n",
                "    mac: macaddr\n",
                "    mac8: macaddr8\n",
                "    fam: IpFamily\n",
                "\n",
                "cfg = NetCfg {\n",
                "    subnet = \"10.0.0.0/24\"\n",
                "    addr = \"10.0.0.10/24\"\n",
                "    mac = \"02:00:00:aa:bb:cc\"\n",
                "    mac8 = \"02:00:00:00:aa:bb:cc:dd\"\n",
                "    fam = net.V6\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");

    let subnet = cfg.dict_get_value("subnet").unwrap();
    assert!(subnet.is_cidr());
    assert_eq!(
        subnet.as_cidr(),
        cidr::IpCidr::from_str("10.0.0.0/24").unwrap()
    );

    let addr = cfg.dict_get_value("addr").unwrap();
    assert!(addr.is_inet());
    assert_eq!(
        addr.as_inet(),
        cidr::IpInet::from_str("10.0.0.10/24").unwrap()
    );

    let mac = cfg.dict_get_value("mac").unwrap();
    assert!(mac.is_macaddr());
    assert_eq!(
        mac.as_macaddr(),
        macaddr::MacAddr6::from_str("02:00:00:aa:bb:cc").unwrap()
    );

    let mac8 = cfg.dict_get_value("mac8").unwrap();
    assert!(mac8.is_macaddr8());
    assert_eq!(
        mac8.as_macaddr8(),
        macaddr::MacAddr8::from_str("02:00:00:00:aa:bb:cc:dd").unwrap()
    );

    let fam = cfg.dict_get_value("fam").unwrap();
    assert!(fam.is_ip_family());
    assert_eq!(fam.as_ip_family(), kcl_runtime::IpFamily::V6);
}

/// F1.7+ — typed protocol-classification predicates on inet.
/// Each is a thin wrapper around the corresponding std-net
/// `Ipv4Addr` / `Ipv6Addr` method dispatched on family. Replaces
/// upstream KCL's stringly-typed `is_*_IP` surface (which F1.7
/// deleted along with the rest of the upstream `net` package).
#[test]
fn mokkan_net_classification_predicates_dispatch_on_family() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "\n",
                "schema Cfg:\n",
                // is_unspecified
                "    unspec_v4: bool\n",
                "    unspec_v4_false: bool\n",
                "    unspec_v6: bool\n",
                // is_loopback
                "    loop_v4: bool\n",
                "    loop_v4_false: bool\n",
                "    loop_v6: bool\n",
                // is_multicast
                "    mcast_v4: bool\n",
                "    mcast_v4_false: bool\n",
                "    mcast_v6: bool\n",
                // is_link_local
                "    ll_v4: bool\n",
                "    ll_v4_false: bool\n",
                "    ll_v6: bool\n",
                "\n",
                "cfg = Cfg {\n",
                "    unspec_v4 = net.is_unspecified(\"0.0.0.0/32\")\n",
                "    unspec_v4_false = net.is_unspecified(\"10.0.0.1/24\")\n",
                "    unspec_v6 = net.is_unspecified(\"::/128\")\n",
                "    loop_v4 = net.is_loopback(\"127.0.0.5/8\")\n",
                "    loop_v4_false = net.is_loopback(\"10.0.0.1/24\")\n",
                "    loop_v6 = net.is_loopback(\"::1/128\")\n",
                "    mcast_v4 = net.is_multicast(\"239.1.2.3/32\")\n",
                "    mcast_v4_false = net.is_multicast(\"10.0.0.1/24\")\n",
                "    mcast_v6 = net.is_multicast(\"ff02::1/128\")\n",
                "    ll_v4 = net.is_link_local(\"169.254.42.42/16\")\n",
                "    ll_v4_false = net.is_link_local(\"10.0.0.1/24\")\n",
                "    ll_v6 = net.is_link_local(\"fe80::1/64\")\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let truthy = [
        "unspec_v4",
        "unspec_v6",
        "loop_v4",
        "loop_v6",
        "mcast_v4",
        "mcast_v6",
        "ll_v4",
        "ll_v6",
    ];
    for k in truthy {
        assert!(
            cfg.dict_get_value(k).unwrap().as_bool(),
            "expected {k} to be true"
        );
    }
    let falsy = [
        "unspec_v4_false",
        "loop_v4_false",
        "mcast_v4_false",
        "ll_v4_false",
    ];
    for k in falsy {
        assert!(
            !cfg.dict_get_value(k).unwrap().as_bool(),
            "expected {k} to be false"
        );
    }
}

/// F1.7 — the three stringly-typed survivors from upstream KCL's
/// deleted `net` package (`fqdn`, `split_host_port`,
/// `join_host_port`) now live under `mokkan.net` per F1.4.bis.
/// Same signatures, same semantics; only the import path changes.
#[test]
fn mokkan_net_stringly_typed_survivors_split_join_roundtrip() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "\n",
                "schema Cfg:\n",
                "    v4_pair: [str]\n",
                "    v6_pair: [str]\n",
                "    v4_joined: str\n",
                "    v6_joined: str\n",
                "\n",
                "cfg = Cfg {\n",
                "    v4_pair = net.split_host_port(\"10.0.0.1:8080\")\n",
                "    v6_pair = net.split_host_port(\"[::1]:8080\")\n",
                "    v4_joined = net.join_host_port(\"10.0.0.1\", \"8080\")\n",
                "    v6_joined = net.join_host_port(\"::1\", \"8080\")\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let v4 = cfg.dict_get_value("v4_pair").unwrap();
    assert_eq!(v4.as_list_ref().values[0].as_str(), "10.0.0.1");
    assert_eq!(v4.as_list_ref().values[1].as_str(), "8080");
    let v6 = cfg.dict_get_value("v6_pair").unwrap();
    assert_eq!(v6.as_list_ref().values[0].as_str(), "::1");
    assert_eq!(v6.as_list_ref().values[1].as_str(), "8080");
    assert_eq!(
        cfg.dict_get_value("v4_joined").unwrap().as_str(),
        "10.0.0.1:8080"
    );
    assert_eq!(
        cfg.dict_get_value("v6_joined").unwrap().as_str(),
        "[::1]:8080"
    );
}

/// F1.7 — upstream `import net` (the stringly-typed package) is
/// gone; `mokkan.net` is the sole networking surface. Importing
/// the dead path now surfaces a resolve-stage diagnostic. Locks
/// down the deletion so a future regression doesn't quietly
/// resurrect the upstream package.
#[test]
fn mokkan_upstream_net_import_path_is_dead() {
    let ready = Embedded::new().build();
    let err = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import net\n",
                "\n",
                "cfg = {\n",
                "    fqdn = net.fqdn()\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("import net should fail post-F1.7");

    let diags = match err {
        EvaluationError::Resolve(d) | EvaluationError::Parse(d) => d,
        other => panic!("expected Resolve/Parse, got: {other:?}"),
    };
    let messages: Vec<&str> = diags
        .iter()
        .flat_map(|d| d.messages.iter().map(|m| m.message.as_str()))
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("net") && m.contains("not found")),
        "expected `pkgpath net not found` diagnostic; got: {messages:?}"
    );
}

// ────────────────────────────────────────────────────────────────────
// F2.5 — `mokkan.net_symbolic` builtins (symbolic-value constructors).
// ────────────────────────────────────────────────────────────────────

/// `import mokkan.net_symbolic` and `net_symbolic.symbolic_subnet(...)`
/// produce a symbolic cidr value at the runtime layer. The Display
/// goes through the F2.1 `CidrValue` diagnostic format — operator-
/// facing stringification flows through the F2.4 ResolvableString
/// path instead.
#[test]
fn mokkan_net_symbolic_subnet_constructs_symbolic_cidr() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "import mokkan.net_symbolic\n",
                "\n",
                "schema NetCfg:\n",
                "    s: cidr\n",
                "\n",
                // Operator-facing call: `symbolic_subnet(handle, size,
                // family)` with family required (D6 "no v4 baked in
                // forever").
                "cfg = NetCfg {\n",
                "    s = net_symbolic.symbolic_subnet(\"lan\", 24, net.V4)\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let s = cfg.dict_get_value("s").unwrap();
    assert_eq!(s.type_str(), "cidr");
    let disp = format!("{s}");
    assert!(
        disp.starts_with("<symbolic cidr: "),
        "F2.1 diagnostic Display for symbolic cidr, got: {disp}"
    );
    // The IR snippet should mention the handle so the diagnostic
    // points at the named network — locks in the debug shape so a
    // regression that loses the handle is caught here.
    assert!(
        disp.contains("\"lan\""),
        "diagnostic should name the handle, got: {disp}"
    );
}

/// `net_symbolic.symbolic_inet(handle, offset)` produces a symbolic
/// inet that downstream algebra (broadcast / + / -) composes via F2.2
/// dispatch. The IR shape is
/// `AddOffset(NetworkOf(HandleSubnet{None,None}), LiteralInt(offset))`
/// per D3 — no size or family on the leaf because the operator
/// asserted neither at the call site.
#[test]
fn mokkan_net_symbolic_inet_constructs_with_addoffset_networkof_handle() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net_symbolic\n",
                "\n",
                "schema NetCfg:\n",
                "    a: inet\n",
                "\n",
                "cfg = NetCfg {\n",
                "    a = net_symbolic.symbolic_inet(\"lan\", 10)\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let a = cfg.dict_get_value("a").unwrap();
    assert_eq!(a.type_str(), "inet");
    let disp = format!("{a}");
    assert!(
        disp.starts_with("<symbolic inet: "),
        "F2.1 diagnostic Display for symbolic inet, got: {disp}"
    );
    // Lock in shape: AddOffset wrapping NetworkOf wrapping
    // HandleSubnet. Each Expr variant name should appear in the
    // debug-shaped diagnostic.
    for needle in ["AddOffset", "NetworkOf", "HandleSubnet", "\"lan\""] {
        assert!(
            disp.contains(needle),
            "expected {needle:?} in IR debug, got: {disp}"
        );
    }
}

/// F2.2 round-trip via F2.5: feed a symbolic inet through
/// `net.broadcast` and check the result is also symbolic, wrapping
/// `BroadcastOf` over the source. This is the first end-to-end
/// exercise of the symbolic-dispatch path through the runtime — the
/// F2.2 unit tests in val_bin.rs covered the operator overloads at
/// the Rust API level; this covers the C-ABI builtin dispatch.
#[test]
fn symbolic_inet_then_net_broadcast_wraps_in_broadcastof() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "import mokkan.net_symbolic\n",
                "\n",
                "schema NetCfg:\n",
                "    b: inet\n",
                "\n",
                // `symbolic_inet("lan", 0)` returns inet (the network's
                // base address), which net.broadcast accepts directly.
                // symbolic_subnet returns cidr and would need a
                // cidr→inet coercion step we don't have.
                "cfg = NetCfg {\n",
                "    b = net.broadcast(net_symbolic.symbolic_inet(\"lan\", 0))\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let b = cfg.dict_get_value("b").unwrap();
    assert_eq!(b.type_str(), "inet");
    let disp = format!("{b}");
    assert!(
        disp.contains("BroadcastOf") && disp.contains("HandleSubnet") && disp.contains("\"lan\""),
        "expected BroadcastOf(... HandleSubnet(\"lan\", ...)) in IR debug, got: {disp}"
    );
}

/// Sema enforces the `family` argument's IpFamily type at
/// resolve-time — passing a string would fail with a type error
/// before the runtime sees it. Locks in the D6 "no v4 baked in
/// forever" rule: family is structurally required.
#[test]
fn symbolic_subnet_rejects_string_family_at_resolve_time() {
    let ready = Embedded::new().build();
    let err = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import mokkan.net_symbolic\n",
                "\n",
                "schema NetCfg:\n",
                "    s: cidr\n",
                "\n",
                "cfg = NetCfg {\n",
                "    s = net_symbolic.symbolic_subnet(\"lan\", 24, \"V4\")\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("string family should fail type check");
    // Any of Resolve/Evaluate is acceptable — the exact stage may
    // shift as sema/evaluator coverage tightens; what matters is
    // that a string in the family slot doesn't silently work.
    match err {
        EvaluationError::Resolve(_) | EvaluationError::Evaluate(_) => {}
        other => panic!("expected Resolve/Evaluate error, got: {other:?}"),
    }
}

// ────────────────────────────────────────────────────────────────────
// F2.6 — `ResolvableString` schema-field type registration + D5 type
// discipline.
// ────────────────────────────────────────────────────────────────────

/// `field: ResolvableString` accepts a bare string literal — the
/// runtime coerces via the F2.6 str→RS path, wrapping in a single
/// Literal segment. Result is fully eager (no symbolic segments) but
/// type-shaped as ResolvableString for downstream codegen.
#[test]
fn resolvable_string_field_accepts_string_literal() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "schema Cfg:\n",
                "    msg: ResolvableString\n",
                "\n",
                "cfg = Cfg {\n",
                "    msg = \"hello\"\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let msg = cfg.dict_get_value("msg").unwrap();
    assert_eq!(msg.type_str(), "ResolvableString");
    // Display concatenates literal segments — single Literal renders
    // as the string itself.
    assert_eq!(format!("{msg}"), "hello");
}

/// `field: str | ResolvableString` is the canonical opt-in shape per
/// F2.6 / D5. A bare string goes through the str arm; an explicit
/// ResolvableString (e.g., from `text(symbolic_inet)`) goes through
/// the RS arm. Both must accept.
#[test]
fn str_or_resolvable_string_field_accepts_both_arms() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "import mokkan.net_symbolic\n",
                "\n",
                "schema Cfg:\n",
                "    eager: str | ResolvableString\n",
                "    deferred: str | ResolvableString\n",
                "\n",
                "cfg = Cfg {\n",
                "    eager = \"plain string\"\n",
                // text(symbolic_inet) produces a single-Symbolic RS,
                // which satisfies the RS arm.
                "    deferred = net.text(net_symbolic.symbolic_inet(\"lan\", 10))\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let eager = cfg.dict_get_value("eager").unwrap();
    let deferred = cfg.dict_get_value("deferred").unwrap();
    // The eager arm may pass through as `str` (matches union arm
    // directly) or be coerced to `ResolvableString` (per the F2.6
    // str→RS unify branch). Either is semantically correct.
    assert!(
        matches!(eager.type_str().as_str(), "str" | "ResolvableString"),
        "unexpected eager type_str: {}",
        eager.type_str()
    );
    // The deferred path produces a ResolvableString from text(symbolic).
    assert_eq!(deferred.type_str(), "ResolvableString");
}

/// F2.6 / D5: multi-segment ResolvableString in a `cidr | ResolvableString`
/// field should be REJECTED. The resolver can substitute a single
/// resolved value for a single Symbolic segment, but multi-segment
/// (Literal + Symbolic + Literal etc.) can't be parsed back as cidr
/// — there's no shape for "literal prefix concatenated with a cidr".
#[test]
fn multi_segment_resolvable_string_rejected_in_non_str_arm() {
    let ready = Embedded::new().build();
    let err = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "import mokkan.net_symbolic\n",
                "\n",
                "schema Cfg:\n",
                // The `cidr | ResolvableString` shape opts in to
                // symbolic-cidr but only for SINGLE-segment RS.
                "    c: cidr | ResolvableString\n",
                "\n",
                "cfg = Cfg {\n",
                // Build a multi-segment RS by concatenating literals
                // with the symbolic stringification. Result has 3
                // segments: Literal(\"prefix-\") + Symbolic(Text(...))
                // + Literal(\"-suffix\"). cidr arm rejects (can't
                // parse), RS arm rejects (multi-segment + non-str
                // other arm).
                "    c = \"prefix-\" + net.text(net_symbolic.symbolic_inet(\"lan\", 10)) + \"-suffix\"\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect_err("multi-segment RS in `cidr | ResolvableString` must reject");
    match err {
        EvaluationError::Resolve(_) | EvaluationError::Evaluate(_) => {}
        other => panic!("expected Resolve/Evaluate error, got: {other:?}"),
    }
}

/// F2.6 / D5: multi-segment ResolvableString in a `str | ResolvableString`
/// field is ACCEPTED — str is "anything-shaped" so concatenation
/// works. The discipline only fires when the non-RS arm is non-str.
#[test]
fn multi_segment_resolvable_string_accepted_in_str_arm() {
    let ready = Embedded::new().build();
    let outcome = evaluate_or_panic(
        &ready,
        EvaluateArgs {
            main_source: concat!(
                "import mokkan.net\n",
                "import mokkan.net_symbolic\n",
                "\n",
                "schema Cfg:\n",
                "    msg: str | ResolvableString\n",
                "\n",
                "cfg = Cfg {\n",
                // Same shape as the previous test — 3-segment RS —
                // but the union's other arm is `str` so the D5
                // discipline allows it.
                "    msg = \"prefix-\" + net.text(net_symbolic.symbolic_inet(\"lan\", 10)) + \"-suffix\"\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        },
    );

    let cfg = outcome.value.dict_get_value("cfg").expect("cfg");
    let msg = cfg.dict_get_value("msg").unwrap();
    assert_eq!(msg.type_str(), "ResolvableString");
}
