//! In-process Rust embedding API for KCL.
//!
//! This crate is the **Rust-direct** evaluation surface for the fork:
//! register schemas in memory, evaluate a user program against them,
//! get a structured [`ValueRef`] tree back. No protobuf serialisation,
//! no JSON/YAML round-trip, no tempdir-on-disk workaround for in-memory
//! schemas. Tilley and other Rust consumers reach for this crate;
//! cross-language consumers (Go, etc.) continue to use the existing
//! `kcl-api` protobuf service surface.
//!
//! See `docs/dev_guide/loader-injection.md` for the design of the
//! virtual-package resolution that backs [`Embedded::register_module`],
//! and `docs/dev_guide/valueref-shape.md` for the [`ValueRef`] contract
//! (notably: not `Send`/`Sync`; do not mutate after evaluation).
//!
//! # Example
//!
//! ```no_run
//! use kcl_embed::{Embedded, EvaluateArgs};
//!
//! let mut embedded = Embedded::new();
//! embedded.register_module(
//!     "tilley",
//!     "schema Vm:\n    name: str\n    memory_mb: int = 1024\n",
//! ).unwrap();
//! let ready = embedded.build();
//!
//! let outcome = ready.evaluate(EvaluateArgs {
//!     main_source: r#"
//!         import tilley
//!         vm = tilley.Vm {name = "alpha"}
//!     "#.to_string(),
//!     ..EvaluateArgs::default()
//! }).unwrap();
//!
//! // outcome.value is a ValueRef tree; walk it directly or feed it
//! // to a kcl-rust-codegen-generated TryFrom impl (Phase 4 work).
//! assert!(outcome.value.is_dict());
//! ```

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use kcl_ast::ast;
use kcl_parser::{KCLModuleCache, LoadProgramOptions, ParseSession, VirtualPackage, load_program};
use kcl_runner::{ExecProgramArgs, FastRunner, RunnerOptions};
use kcl_runtime::ValueRef;
use kcl_sema::resolver::resolve_program;

/// The diagnostic type used throughout this crate's error surface.
///
/// Re-exported from `kcl-error` rather than wrapped. Callers can inspect
/// the [`Diagnostic`]'s messages, ranges, and severity directly. If a
/// future contributor wants to introduce a `kcl_embed::Diagnostic`
/// wrapper for API hygiene, that's a breaking change — document the
/// reasons in this crate's README before flipping.
pub use kcl_error::Diagnostic;

/// Synthetic-path prefix used as the `pkg_root` for in-memory modules.
/// Conventional only; the parser treats it as an opaque identifier.
/// Exposed primarily for debugging/diagnostics where a path string
/// makes it visible that the source came from an in-memory registry.
pub const VIRTUAL_PACKAGE_ROOT_PREFIX: &str = "/__kcl_embed__";

/// Builder phase of the embedding API.
///
/// Register host-provided modules with [`Embedded::register_module`],
/// then call [`Embedded::build`] to freeze the registry and produce
/// an [`EmbeddedReady`]. Once built, no further modules can be
/// registered against that handle — typestate enforces this at
/// compile time. To change the registered set, construct a new
/// [`Embedded`].
///
/// **Concurrency:** `Embedded` is `!Send` and `!Sync` — it holds an
/// `Rc<RefCell<...>>`-shaped backing store inherited from the
/// underlying KCL runtime's value model. See
/// `docs/dev_guide/valueref-shape.md` for the broader contract.
#[derive(Debug, Default)]
pub struct Embedded {
    /// Map from package name to in-memory module source. Populated by
    /// [`Embedded::register_module`].
    modules: HashMap<String, String>,
}

/// Evaluator-ready, frozen embedding handle. Produced by
/// [`Embedded::build`]. Only [`EmbeddedReady::evaluate`] is reachable
/// from this type; modules cannot be added or removed.
#[derive(Debug)]
pub struct EmbeddedReady {
    /// `kcl_parser::VirtualPackage` map ready to drop into
    /// `LoadProgramOptions::virtual_packages`. Built once at
    /// [`Embedded::build`] time so subsequent `evaluate` calls don't
    /// rebuild the synthetic-path layout.
    virtual_packages: HashMap<String, VirtualPackage>,
}

/// Arguments controlling a single [`EmbeddedReady::evaluate`] call.
///
/// Each field maps to a KCL CLI flag: `main_source` is the
/// stdin-equivalent program, `work_dir` is `--work-dir`,
/// `external_args` is `-D key=value`, `external_packages` is
/// `-E pkg=path`, `overrides` is `-O`, `path_selector` is `-S`.
/// File-based `-Y settings.yaml` ingestion is intentionally **out of
/// scope** for v1; consumers needing it synthesise the equivalent
/// through these typed fields.
#[derive(Debug, Default)]
pub struct EvaluateArgs {
    /// Top-level KCL program source — the equivalent of the file that
    /// would be passed positionally to `kcl run`. Required (no default).
    pub main_source: String,
    /// Working directory for KCL's mod-relative path resolution and
    /// vendor-package lookups. `None` means use the process cwd.
    pub work_dir: Option<PathBuf>,
    /// `-D key=value`-style external arguments accessible via the
    /// `option()` builtin from user code.
    pub external_args: Vec<(String, String)>,
    /// `-E pkg=path`-style disk-package surface. Resolves *after*
    /// in-memory registered modules (per the embed API's documented
    /// shadowing contract); see `docs/dev_guide/loader-injection.md`
    /// for the resolution flow.
    pub external_packages: HashMap<String, PathBuf>,
    /// `-O`-style override specs.
    pub overrides: Vec<String>,
    /// `-S`-style path selectors for filtering the result.
    pub path_selector: Vec<String>,
}

/// Successful outcome of an [`EmbeddedReady::evaluate`] call.
///
/// `value` is the dict-merged global scope as a structured
/// [`ValueRef`] (see `kcl_runtime::ValueRef` and
/// `docs/dev_guide/valueref-shape.md`). `log_messages` is the
/// accumulated output of `print()` calls in the user program — the
/// library does not write to stderr by default.
#[derive(Debug)]
pub struct EvaluateOutcome {
    /// Structured result tree. Walk it directly or feed to a
    /// `kcl-rust-codegen`-emitted `TryFrom<&ValueRef>` impl (Phase 4
    /// of the restructuring plan).
    pub value: ValueRef,
    /// `print()` output collected during evaluation, one entry per
    /// emission. Callers decide what to do with these (log them, drop
    /// them, surface them in a UI). The library does not implicitly
    /// emit to stdout or stderr.
    pub log_messages: Vec<String>,
}

/// Errors produced by [`EmbeddedReady::evaluate`].
///
/// Each variant carries the diagnostics directly so callers can
/// inspect every parse/resolve/evaluate problem in one pass without
/// re-parsing a string-encoded message.
#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    /// Parse-stage errors: lexer or parser problems in either the
    /// main source or any imported module.
    #[error("KCL parse error ({} diagnostics)", .0.len())]
    Parse(Vec<Diagnostic>),

    /// Sema/resolver-stage errors: type checking, symbol resolution,
    /// import-graph problems. Includes "pkgpath not found" for
    /// unregistered imports.
    #[error("KCL resolve error ({} diagnostics)", .0.len())]
    Resolve(Vec<Diagnostic>),

    /// Evaluation-stage errors: check-block violations, runtime
    /// asserts, schema attribute mismatches discovered during
    /// instantiation.
    #[error("KCL evaluation error ({} diagnostics)", .0.len())]
    Evaluate(Vec<Diagnostic>),

    /// VM-internal failure (panic bridge, unexpected state, etc.).
    ///
    /// Downstream consumers should treat this as non-recoverable and
    /// report it; **do not write recovery logic against the inner
    /// error type**. The boxed `dyn Error` is intentionally opaque to
    /// discourage downcast-based fragility against KCL-internal
    /// types that may change between revisions.
    #[error("KCL internal failure: {0}")]
    Internal(Box<dyn std::error::Error + Send + Sync + 'static>),
}

/// Errors produced by [`Embedded::register_module`].
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// A module with this path has already been registered against
    /// this [`Embedded`] handle. Explicit by design — silent
    /// last-write-wins would mask programmer error. Callers needing
    /// replace-semantics should construct a fresh [`Embedded`].
    #[error("module {0:?} already registered")]
    AlreadyRegistered(String),
}

impl Embedded {
    /// Construct a new, empty [`Embedded`] builder. No modules
    /// registered; no host-provided sources visible to the evaluator
    /// until [`Embedded::register_module`] is called.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an in-memory module addressable by the given KCL
    /// module path. `source` is the verbatim text of the module
    /// (anything `kcl-parser` accepts for a `.k` file).
    ///
    /// **Semantics** (matching `docs/dev_guide/loader-injection.md`):
    /// - Lazy: `source` is not parsed at register time; it's parsed
    ///   on first `import` resolution during a subsequent
    ///   [`EmbeddedReady::evaluate`] call.
    /// - Shadowing: a registered module wins over any same-named
    ///   on-disk external package the evaluator would otherwise
    ///   find through `external_packages` or the vendor dirs.
    /// - Transitive imports inside `source` resolve through the same
    ///   registry first, then fall back to disk via the existing
    ///   loader machinery.
    /// - Collision: registering the same `module_path` twice on the
    ///   same builder returns
    ///   [`EmbedError::AlreadyRegistered`]; replace-semantics is not
    ///   supported in v1.
    pub fn register_module(
        &mut self,
        module_path: &str,
        source: &str,
    ) -> Result<(), EmbedError> {
        if self.modules.contains_key(module_path) {
            return Err(EmbedError::AlreadyRegistered(module_path.to_string()));
        }
        self.modules
            .insert(module_path.to_string(), source.to_string());
        Ok(())
    }

    /// Freeze the registry and produce an evaluator-ready
    /// [`EmbeddedReady`]. After this call, no further modules can be
    /// registered against the moved-from [`Embedded`] — that's the
    /// typestate's point.
    ///
    /// Each registered `(module_path, source)` becomes a
    /// [`VirtualPackage`] with a synthetic root of
    /// `/__kcl_embed__/<module_path>` and a single synthetic file at
    /// `/__kcl_embed__/<module_path>/main.k`. The evaluator never
    /// touches those paths on disk; they're opaque identifiers for
    /// the parser's import-resolution and module-cache layers.
    pub fn build(self) -> EmbeddedReady {
        let mut virtual_packages = HashMap::with_capacity(self.modules.len());
        for (module_path, source) in self.modules {
            let root = PathBuf::from(format!("{VIRTUAL_PACKAGE_ROOT_PREFIX}/{module_path}"));
            let main_file = root.join("main.k");
            virtual_packages.insert(
                module_path,
                VirtualPackage {
                    root,
                    files: vec![(main_file, source)],
                },
            );
        }
        EmbeddedReady { virtual_packages }
    }
}

/// Synthetic path used for the in-memory main program when an
/// `EvaluateArgs::main_source` is provided. The KCL parser pairs this
/// with `k_code_list` entries during compile-entry construction (see
/// `kcl_parser::get_compile_entries_from_paths`); the path string is
/// opaque to the parser apart from being used as the source's
/// reported filename in diagnostics.
const VIRTUAL_MAIN_FILENAME: &str = "__kcl_embed_main__.k";

impl EmbeddedReady {
    /// Evaluate a KCL program against the frozen module registry.
    ///
    /// `args.main_source` is the top-level program text. Any `import`
    /// statements in it (or in registered modules transitively
    /// imported from it) resolve through the registry first, then
    /// fall back to disk via the existing loader machinery. The
    /// returned [`EvaluateOutcome::value`] is the dict-merged global
    /// scope as a structured [`ValueRef`]; see
    /// `docs/dev_guide/valueref-shape.md` for the contract.
    ///
    /// **Panic bridging:** evaluation is wrapped in
    /// `std::panic::catch_unwind(AssertUnwindSafe(...))`. The
    /// `AssertUnwindSafe` is deliberate — the underlying evaluator
    /// holds `Rc<RefCell<...>>` and is not `UnwindSafe`, but the
    /// state is dropped immediately after a caught panic so the
    /// obligation is satisfied. Phase 6a will refine this from a
    /// blanket bridge into per-site `Result` propagation; until then,
    /// caught panics surface as [`EvaluationError::Internal`].
    ///
    /// **Diagnostic granularity:** parse errors surface as
    /// [`EvaluationError::Parse`], sema/resolve errors as
    /// [`EvaluationError::Resolve`], and runtime evaluation failures
    /// (check-block violations, asserts, runtime type mismatches) as
    /// [`EvaluationError::Evaluate`]. The runtime-failure case
    /// currently surfaces as a single best-effort [`Diagnostic`]
    /// derived from the runner's `err_message` string — Phase 6a
    /// will swap this for direct structured propagation.
    pub fn evaluate(
        &self,
        args: EvaluateArgs,
    ) -> Result<EvaluateOutcome, EvaluationError> {
        let exec_args = build_exec_args(&args);
        let load_opts = self.build_load_options(&args);
        let module_cache = self.build_module_cache();

        let sess = Arc::new(ParseSession::default());

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            evaluate_inner(sess.clone(), exec_args, load_opts, module_cache)
        }));

        match outcome {
            Ok(result) => result,
            Err(panic_payload) => {
                let msg = kcl_error::err_to_str(panic_payload);
                Err(EvaluationError::Internal(Box::new(
                    BridgedPanicError(msg),
                )))
            }
        }
    }

    /// Number of registered virtual packages. Exposed for tests and
    /// diagnostics; not part of the evaluation surface.
    pub fn registered_module_count(&self) -> usize {
        self.virtual_packages.len()
    }

    /// Build the `LoadProgramOptions` for this evaluation, threading
    /// the frozen `virtual_packages` map plus any caller-provided
    /// `external_packages` for disk-resolved packages.
    fn build_load_options(&self, args: &EvaluateArgs) -> LoadProgramOptions {
        let mut package_maps = HashMap::new();
        for (name, path) in &args.external_packages {
            package_maps.insert(name.clone(), path.to_string_lossy().to_string());
        }
        LoadProgramOptions {
            work_dir: args
                .work_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            k_code_list: vec![args.main_source.clone()],
            package_maps,
            virtual_packages: self.virtual_packages.clone(),
            ..Default::default()
        }
    }

    /// Build a `KCLModuleCache` pre-populated with `source_code`
    /// entries for every synthetic file path the registry advertises.
    /// `parse_file`'s lookup at `crates/parser/src/lib.rs:704–712`
    /// consults this cache before falling back to a disk read,
    /// so the synthetic paths resolve to in-memory source without
    /// any further plumbing.
    fn build_module_cache(&self) -> KCLModuleCache {
        let cache = KCLModuleCache::default();
        if let Ok(mut cache_w) = cache.write() {
            for vp in self.virtual_packages.values() {
                for (path, source) in &vp.files {
                    cache_w
                        .source_code
                        .insert(path.clone(), source.clone());
                }
            }
        }
        cache
    }
}

/// Build the `ExecProgramArgs` shape the runner consumes, mapping
/// `EvaluateArgs`' typed fields into the existing CLI-aligned arg
/// schema.
fn build_exec_args(args: &EvaluateArgs) -> ExecProgramArgs {
    let mut exec_args = ExecProgramArgs {
        work_dir: args
            .work_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        k_filename_list: vec![VIRTUAL_MAIN_FILENAME.to_string()],
        k_code_list: vec![args.main_source.clone()],
        overrides: args.overrides.clone(),
        path_selector: args.path_selector.clone(),
        ..Default::default()
    };
    exec_args.args = args
        .external_args
        .iter()
        .map(|(name, value)| ast::Argument {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    exec_args.set_external_pkg_from_package_maps(
        args.external_packages
            .iter()
            .map(|(name, path)| (name.clone(), path.to_string_lossy().to_string()))
            .collect(),
    );
    exec_args
}

/// The non-`catch_unwind` body of [`EmbeddedReady::evaluate`]. Split
/// out so the panic bridge sits cleanly at the call site without
/// burying the happy-path flow inside an `AssertUnwindSafe(|| { ... })`.
fn evaluate_inner(
    sess: Arc<ParseSession>,
    exec_args: ExecProgramArgs,
    load_opts: LoadProgramOptions,
    module_cache: KCLModuleCache,
) -> Result<EvaluateOutcome, EvaluationError> {
    // Stage 1: parse. load_program collects diagnostics on the
    // session's Handler; surface them as EvaluationError::Parse if
    // any errors were recorded.
    let main_path = VIRTUAL_MAIN_FILENAME;
    let parse_result = load_program(
        sess.clone(),
        &[main_path],
        Some(load_opts),
        Some(module_cache),
    )
    .map_err(|e| {
        EvaluationError::Internal(Box::new(BridgedAnyhowError(e.to_string())))
    })?;

    let (parse_errors, _warnings) = sess.classification();
    if !parse_errors.is_empty() {
        return Err(EvaluationError::Parse(
            parse_errors.into_iter().collect(),
        ));
    }
    // load_program may also stash parse errors on parse_result.errors
    // (separately from the session Handler). Surface both.
    if !parse_result.errors.is_empty() {
        return Err(EvaluationError::Parse(
            parse_result.errors.into_iter().collect(),
        ));
    }

    // Stage 2: sema (resolve). resolve_program writes diagnostics
    // onto its scope.handler; check for errors before evaluation.
    let mut program = parse_result.program;
    let scope = resolve_program(&mut program);
    let (resolve_errors, _resolve_warnings) = scope.handler.classification();
    if !resolve_errors.is_empty() {
        return Err(EvaluationError::Resolve(
            resolve_errors.into_iter().collect(),
        ));
    }

    // Stage 3: evaluation. FastRunner::run_to_value runs the program
    // and catches runtime panics internally; runtime failures
    // surface as a non-empty err_message on the result.
    let runner = FastRunner::new(Some(RunnerOptions {
        plugin_agent_ptr: exec_args.plugin_agent,
    }));
    let runner_result = runner.run_to_value(&program, &exec_args).map_err(|e| {
        EvaluationError::Internal(Box::new(BridgedAnyhowError(e.to_string())))
    })?;

    let log_messages = split_log_messages(&runner_result.log_message);

    if !runner_result.err_message.is_empty() {
        // Phase 6a will replace this with direct Diagnostic
        // propagation. For now, surface the err_message as a single
        // best-effort Diagnostic in EvaluationError::Evaluate.
        return Err(EvaluationError::Evaluate(vec![
            diagnostic_from_runner_err(&runner_result.err_message),
        ]));
    }

    Ok(EvaluateOutcome {
        value: runner_result.value,
        log_messages,
    })
}

/// Split the runner's accumulated `log_message` string into individual
/// entries. KCL's `print()` builtin appends each call's output
/// followed by a newline; we split on `\n` and drop the trailing
/// empty string.
fn split_log_messages(buffer: &str) -> Vec<String> {
    if buffer.is_empty() {
        return Vec::new();
    }
    buffer
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

/// Construct a best-effort [`Diagnostic`] from the runner's
/// `err_message`. Used in the runtime-failure path until Phase 6a
/// wires structured propagation.
fn diagnostic_from_runner_err(message: &str) -> Diagnostic {
    use kcl_error::{DiagnosticId, Level, Message, Position, Style};
    Diagnostic {
        level: Level::Error,
        messages: vec![Message {
            range: (Position::dummy_pos(), Position::dummy_pos()),
            style: Style::Line,
            message: message.to_string(),
            note: None,
            suggested_replacement: None,
        }],
        code: Some(DiagnosticId::Error(
            kcl_error::ErrorKind::EvaluationError,
        )),
    }
}

/// Wrapper that lets the `Internal` arm of [`EvaluationError`] carry
/// an `anyhow::Error`'s message without entangling the public surface
/// with the `anyhow` crate.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct BridgedAnyhowError(String);

/// Wrapper that lets the `Internal` arm of [`EvaluationError`] carry
/// a panic payload's message string.
#[derive(Debug, thiserror::Error)]
#[error("evaluator panicked: {0}")]
struct BridgedPanicError(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_build_collects_virtual_packages() {
        let mut embedded = Embedded::new();
        embedded
            .register_module("tilley", "schema Vm:\n    name: str\n")
            .expect("first register should succeed");
        embedded
            .register_module("tilley_vm", "schema VmSpec:\n    cores: int = 2\n")
            .expect("second register should succeed");
        let ready = embedded.build();

        assert_eq!(ready.registered_module_count(), 2);

        // Synthetic root convention is observable to callers — pin it
        // here so a change to the prefix is a deliberate breaking
        // edit, not an accidental rename.
        let tilley_root = format!("{VIRTUAL_PACKAGE_ROOT_PREFIX}/tilley");
        assert_eq!(
            ready
                .virtual_packages
                .get("tilley")
                .expect("tilley pkg")
                .root
                .to_string_lossy(),
            tilley_root
        );
    }

    #[test]
    fn duplicate_register_returns_already_registered() {
        let mut embedded = Embedded::new();
        embedded
            .register_module("tilley", "schema Vm:\n    name: str\n")
            .expect("first register should succeed");
        let err = embedded
            .register_module("tilley", "schema Other:\n    x: int\n")
            .expect_err("duplicate register should fail");
        match err {
            EmbedError::AlreadyRegistered(path) => assert_eq!(path, "tilley"),
        }
    }

    #[test]
    fn evaluate_inline_main_produces_structured_value() {
        // End-to-end smoke: a main source with no imports evaluates
        // and surfaces a structured ValueRef. The full
        // import-from-registered-module flow is exercised by the
        // integration tests in step 5.
        let ready = Embedded::new().build();
        let outcome = ready
            .evaluate(EvaluateArgs {
                main_source: "alice = {age = 18}".to_string(),
                ..EvaluateArgs::default()
            })
            .expect("evaluate should succeed");
        assert!(
            outcome.value.is_dict(),
            "top-level value should be a dict, got type={}",
            outcome.value.type_str()
        );
        let alice = outcome
            .value
            .dict_get_value("alice")
            .expect("alice should be in top-level dict");
        assert!(alice.is_dict());
        assert_eq!(alice.dict_get_value("age").unwrap().as_int(), 18);
        assert!(outcome.log_messages.is_empty());
    }
}
