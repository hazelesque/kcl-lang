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
use std::path::PathBuf;

use kcl_parser::VirtualPackage;
use kcl_runtime::ValueRef;

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

impl EmbeddedReady {
    /// Evaluate a KCL program against the frozen module registry.
    ///
    /// **Not yet implemented** in this step of the restructuring; the
    /// loader-side wiring (Phase 3 step 2) and this crate's public
    /// surface (step 3) are landed first so the API shape is
    /// reviewable before the evaluation plumbing is wired through.
    /// Phase 3 step 4 (the next commit) connects this to
    /// `kcl_runner::FastRunner::run_to_value` via a `catch_unwind`
    /// bridge.
    pub fn evaluate(
        &self,
        _args: EvaluateArgs,
    ) -> Result<EvaluateOutcome, EvaluationError> {
        Err(EvaluationError::Internal(Box::new(NotYetImplemented)))
    }

    /// Number of registered virtual packages. Exposed for tests and
    /// diagnostics; not part of the evaluation surface.
    pub fn registered_module_count(&self) -> usize {
        self.virtual_packages.len()
    }
}

/// Sentinel error type used by the step-3 stub [`EmbeddedReady::evaluate`]
/// until step 4 lands the full implementation.
#[derive(Debug, thiserror::Error)]
#[error("kcl-embed: EmbeddedReady::evaluate is not yet wired through to the runner")]
struct NotYetImplemented;

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
    fn evaluate_is_a_step3_stub() {
        // Lock the contract: step 3 ships the surface; step 4 wires
        // evaluation. Until step 4 lands, evaluate returns Internal.
        let ready = Embedded::new().build();
        let outcome = ready.evaluate(EvaluateArgs::default());
        match outcome {
            Err(EvaluationError::Internal(_)) => { /* step-3 contract */ }
            other => panic!(
                "step-3 stub should return EvaluationError::Internal; got: {other:?}"
            ),
        }
    }
}
