//! Generate Rust types + `TryFrom<&ValueRef>` impls from KCL schemas.
//!
//! The Phase 4 piece of the structuring plan: takes KCL schema source
//! (a single inline string or a list of file paths), parses + resolves
//! it through `kcl-parser` + `kcl-sema`, walks the resolved schemas
//! into an internal IR, and emits Rust source text.
//!
//! Generated types implement `TryFrom<&kcl_runtime::ValueRef>` per
//! the D4 seatbelt contract: `Err` is only returned when the input
//! `ValueRef` does not match the expected schema shape, and in a
//! well-formed deployment that branch is unreachable (the VM would
//! have rejected the input before producing a non-conforming
//! `ValueRef`).
//!
//! # Phase 4 MVP scope (per the plan)
//!
//! Supported:
//! - Primitive types: `int`, `float`, `bool`, `str`
//! - Lists (`[T]`) and dicts (`{K:V}`)
//! - Schema definitions with `pub` field structs
//! - Optional fields (`field?: T`) mapping to `Option<T>`
//!   - Per D5 (empirically resolved in commit abecf243):
//!     `Value::undefined` and `Value::none` both map to `None`
//! - Mixins (transparent — per D10 in
//!   `docs/dev_guide/codegen-investigation.md`)
//! - Unions (D6: disjoint structural, primitive-structural,
//!   discriminator-required, and the single-schema-with-discriminator
//!   `# @rust: tagged_enum(discriminator = "...")` annotation)
//!
//! Codegen-time error (deferred until a consumer needs them):
//! - Schema inheritance (`schema Child(Parent):`) — D9 deferral
//! - Function-typed fields — D4 corollary, FuncValue bear-trap
//! - Unit-typed values — not in MVP, no Tilley consumer
//! - Unresolvable union disambiguation per D6
//!
//! # Example
//!
//! ```no_run
//! use kcl_rust_codegen::generate_to_string;
//!
//! let kcl_source = "schema Vm:\n    name: str\n    memory_mb: int = 1024\n";
//! let rust_source = generate_to_string(kcl_source).unwrap();
//! // Write rust_source to OUT_DIR in a build.rs, include! it, use the
//! // generated `Vm` struct + TryFrom impl in your code.
//! ```

use std::sync::Arc;

use kcl_parser::{ParseSession, load_program};
use kcl_sema::resolver::resolve_program;

mod annotation;
mod emit;
mod ir;

pub use ir::{EnumIR, FieldIR, FieldKind, ModuleIR, SchemaIR};

/// The diagnostic type used by [`CodegenError`]; re-exported so
/// callers can inspect ranges and messages without depending on
/// `kcl_error` directly. (Mirrors the embed crate's re-export
/// decision.)
pub use kcl_error::Diagnostic;

/// Errors that [`generate_to_string`] / [`generate_from_files`] can
/// surface. Multi-diagnostic-carrying for parse and resolve stages so
/// every problem is visible in one pass.
#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    /// Parse-stage errors in the KCL source.
    #[error("KCL parse error ({} diagnostics)", .0.len())]
    Parse(Vec<Diagnostic>),

    /// Sema/resolve-stage errors. Includes "pkgpath not found" for
    /// imports that the codegen tool doesn't know how to satisfy.
    #[error("KCL resolve error ({} diagnostics)", .0.len())]
    Resolve(Vec<Diagnostic>),

    /// The schema source uses a feature that the Phase 4 MVP does
    /// not codegen for. Carries the feature name + a human-readable
    /// location for the offending construct. See the crate-level
    /// docs for which features are deferred and why.
    #[error("unsupported feature `{feature}` at {location}")]
    UnsupportedFeature {
        /// Short, machine-parseable feature identifier
        /// (e.g. `"schema_inheritance"`, `"function_field"`,
        /// `"unit_typed_value"`).
        feature: &'static str,
        /// Human-readable location string, e.g. `"schema Vm
        /// (test.k:3:1)"`.
        location: String,
    },

    /// A `# @rust: ...` annotation on a schema declaration didn't
    /// parse, was malformed, or referenced a discriminator field
    /// that doesn't exist on the annotated schema.
    #[error("invalid `# @rust` annotation: {0}")]
    InvalidAnnotation(String),

    /// Catch-all for internal errors that don't fit the other
    /// variants. Should not be reached by well-formed input;
    /// reaching it indicates a codegen bug.
    #[error("kcl-rust-codegen internal: {0}")]
    Internal(String),
}

/// Generate Rust source text from an inline KCL schema source string.
///
/// The input is treated as the contents of a single top-level
/// module. Schemas declared in it become public types in the output;
/// `import` statements are not currently supported (codegen runs
/// against a single self-contained source).
///
/// Returns the generated Rust source as a `String`. Consumers
/// typically write this to `$OUT_DIR/<name>.rs` from a `build.rs`
/// and `include!` it in their crate.
pub fn generate_to_string(kcl_source: &str) -> Result<String, CodegenError> {
    let module = analyse_inline_source(kcl_source)?;
    emit_rust_source(&module)
}

/// Walk a KCL source string through parser + sema and extract the
/// [`ModuleIR`] (schemas + lifted enums). Mostly for testing /
/// introspection; production consumers use [`generate_to_string`].
pub fn analyse_inline_source(kcl_source: &str) -> Result<ModuleIR, CodegenError> {
    let sess = Arc::new(ParseSession::default());
    let main_path = "__kcl_codegen_input__.k";

    let opts = kcl_parser::LoadProgramOptions {
        k_code_list: vec![kcl_source.to_string()],
        ..Default::default()
    };
    let parse_result = load_program(
        sess.clone(),
        &[main_path],
        Some(opts),
        Some(kcl_parser::KCLModuleCache::default()),
    )
    .map_err(|e| CodegenError::Internal(e.to_string()))?;

    let (parse_errors, _) = sess.classification();
    if !parse_errors.is_empty() {
        return Err(CodegenError::Parse(parse_errors.into_iter().collect()));
    }
    if !parse_result.errors.is_empty() {
        return Err(CodegenError::Parse(
            parse_result.errors.into_iter().collect(),
        ));
    }

    let mut program = parse_result.program;
    let scope = resolve_program(&mut program);
    let (resolve_errors, _) = scope.handler.classification();
    if !resolve_errors.is_empty() {
        return Err(CodegenError::Resolve(resolve_errors.into_iter().collect()));
    }

    ir::extract_module(&program, &scope)
}

/// Emit Rust source text from a [`ModuleIR`].
fn emit_rust_source(module: &ModuleIR) -> Result<String, CodegenError> {
    emit::emit_rust_source(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::pascal_case;

    #[test]
    fn pascal_case_handles_snake_case() {
        assert_eq!(pascal_case("audit_id"), "AuditId");
        assert_eq!(pascal_case("port"), "Port");
        assert_eq!(pascal_case("a_b_c"), "ABC");
    }

    #[test]
    fn pascal_case_handles_kebab_case() {
        assert_eq!(pascal_case("virtio-serial"), "VirtioSerial");
        assert_eq!(pascal_case("isa-serial"), "IsaSerial");
        assert_eq!(pascal_case("foo-bar-baz"), "FooBarBaz");
    }

    #[test]
    fn pascal_case_handles_space_separated() {
        assert_eq!(pascal_case("foo bar"), "FooBar");
        assert_eq!(pascal_case("hello world again"), "HelloWorldAgain");
    }

    #[test]
    fn pascal_case_handles_mixed_separators() {
        // KCL string-literal-union values can carry any printable
        // character; the codegen needs a valid Rust identifier out.
        assert_eq!(pascal_case("foo-bar_baz qux"), "FooBarBazQux");
        assert_eq!(pascal_case("a.b/c"), "ABC");
    }

    #[test]
    fn pascal_case_is_idempotent_on_already_pascal_input() {
        assert_eq!(pascal_case("VirtioSerial"), "VirtioSerial");
        assert_eq!(pascal_case("Port"), "Port");
    }

    #[test]
    fn pascal_case_preserves_internal_digits() {
        assert_eq!(pascal_case("v1"), "V1");
        assert_eq!(pascal_case("ip6_only"), "Ip6Only");
    }

    #[test]
    fn analyse_simple_schema_produces_one_schema_ir() {
        let src = concat!(
            "schema Vm:\n",
            "    name: str\n",
            "    memory_mb: int = 1024\n",
            "    notes?: str\n",
        );
        let module = analyse_inline_source(src).expect("analyse should succeed");
        assert_eq!(module.schemas.len(), 1);
        let vm = &module.schemas[0];
        assert_eq!(vm.name, "Vm");
        assert_eq!(vm.fields.len(), 3);
        // Field order from KCL source is preserved through sema's
        // IndexMap, which we propagate to FieldIR. Consumers rely on
        // declaration order for ergonomic generated structs.
        assert_eq!(vm.fields[0].name, "name");
        assert!(!vm.fields[0].optional);
        assert!(!vm.fields[0].has_default);
        assert_eq!(vm.fields[1].name, "memory_mb");
        assert!(vm.fields[1].has_default);
        assert_eq!(vm.fields[2].name, "notes");
        assert!(vm.fields[2].optional);
    }

    #[test]
    fn analyse_lifts_string_literal_union_to_enum() {
        let src = concat!(
            "schema TestAssertion:\n",
            "    type: \"command\" | \"service\" | \"port\"\n",
            "    command?: str\n",
        );
        let module = analyse_inline_source(src).expect("analyse");
        assert_eq!(module.schemas.len(), 1);
        assert_eq!(module.enums.len(), 1, "expected one lifted enum");
        let lifted = &module.enums[0];
        assert_eq!(lifted.rust_name, "TestAssertionType");
        assert_eq!(lifted.variants, vec!["command", "service", "port"]);
        assert_eq!(lifted.origin, "TestAssertion.type");
        let type_field = &module.schemas[0].fields[0];
        assert_eq!(type_field.name, "type");
        assert!(matches!(
            &type_field.kind,
            FieldKind::StrEnum(name) if name == "TestAssertionType"
        ));
    }

    #[test]
    fn generate_to_string_emits_banner_struct_and_tryfrom() {
        let src = concat!(
            "schema Vm:\n",
            "    name: str\n",
            "    memory_mb: int = 1024\n",
            "    notes?: str\n",
        );
        let out = generate_to_string(src).expect("generate should succeed");
        assert!(out.contains("AUTO-GENERATED by kcl-rust-codegen"));
        assert!(out.contains("pub struct Vm {"));
        assert!(out.contains("pub name: String,"));
        assert!(out.contains("pub memory_mb: i64,"));
        assert!(out.contains("pub notes: Option<String>,"));
        assert!(out.contains("impl TryFrom<&kcl_runtime::ValueRef> for Vm"));
    }

    #[test]
    fn parse_error_surfaces_as_codegen_parse_error() {
        // Deliberately broken syntax that fails the lexer/parser.
        let src = "schema ::Broken (not a real syntax)";
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::Parse(diags) | CodegenError::Resolve(diags) => {
                assert!(!diags.is_empty());
            }
            other => panic!("expected Parse/Resolve, got: {other:?}"),
        }
    }

    /// Schema inheritance is deferred from Phase 4 MVP (D9). Codegen
    /// should refuse with a specific UnsupportedFeature error that
    /// names "schema_inheritance" — not silently produce wrong code.
    #[test]
    fn schema_inheritance_errors_with_actionable_feature_name() {
        let src = concat!(
            "schema Parent:\n",
            "    base_field: int\n",
            "\n",
            "schema Child(Parent):\n",
            "    child_field: str\n",
        );
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::UnsupportedFeature { feature, location } => {
                assert_eq!(feature, "schema_inheritance");
                assert!(
                    location.contains("Child"),
                    "location should name the offending schema; got: {location}"
                );
            }
            other => panic!("expected UnsupportedFeature, got: {other:?}"),
        }
    }

    /// Non-string-literal unions (e.g. `int | str`) are deferred
    /// pending the tagged_enum annotation work. Codegen should
    /// refuse with a specific message that points at the workaround,
    /// not produce a broken `()` type.
    #[test]
    fn non_string_literal_union_errors_with_actionable_message() {
        let src = concat!("schema Spec:\n", "    quota: int | str\n",);
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::UnsupportedFeature { feature, location } => {
                assert_eq!(feature, "field_kind");
                assert!(
                    location.contains("union")
                        || location.contains("Union")
                        || location.contains("tagged_enum"),
                    "message should point at the workaround; got: {location}"
                );
            }
            other => panic!("expected UnsupportedFeature, got: {other:?}"),
        }
    }

    /// Phase 4B step 1+2: schema-level `# @rust: tagged_enum(...)`
    /// and per-field `# @rust: variant(...)` / `# @rust: shared`
    /// annotations are parsed AND the schema is promoted to a
    /// `TaggedEnumIR` (lifted out of `module.schemas` into
    /// `module.tagged_enums`).
    #[test]
    fn tagged_enum_annotations_produce_tagged_enum_ir() {
        // Two-space indent in the source matters: KCL's parser
        // tracks columns, and the trailing-comment matcher uses
        // line equality. The annotation comments live on the same
        // line as their fields.
        let src = concat!(
            "# @rust: tagged_enum(discriminator = \"type\")\n",
            "schema TestAssertion:\n",
            "    type: \"command\" | \"service\"\n",
            "    description?: str  # @rust: shared\n",
            "    command?: str  # @rust: variant(\"command\")\n",
            "    service?: str  # @rust: variant(\"service\")\n",
        );
        let module = analyse_inline_source(src).expect("analyse");
        // The annotated schema was promoted out of schemas into
        // tagged_enums.
        assert_eq!(module.schemas.len(), 0, "annotated schema should be lifted");
        assert_eq!(module.tagged_enums.len(), 1, "expected one tagged enum");
        // The lifted enum for the discriminator field is also gone
        // from module.enums — the variant set is now embedded in the
        // TaggedEnumIR.
        assert_eq!(
            module.enums.len(),
            0,
            "discriminator's lifted str-enum should be removed; got: {:?}",
            module.enums
        );

        let t = &module.tagged_enums[0];
        assert_eq!(t.name, "TestAssertion");
        assert_eq!(t.discriminator, "type");
        assert_eq!(t.variants.len(), 2);
        // Variant order follows the discriminator literal's source
        // order in the KCL union.
        assert_eq!(t.variants[0].kcl_literal, "command");
        assert_eq!(t.variants[0].rust_name, "Command");
        assert_eq!(t.variants[0].fields.len(), 1);
        assert_eq!(t.variants[0].fields[0].name, "command");
        // Within an assigned variant the field is required (the
        // schema's check block enforces this), so the optional flag
        // is collapsed.
        assert!(!t.variants[0].fields[0].optional);
        assert_eq!(t.variants[1].kcl_literal, "service");
        assert_eq!(t.variants[1].rust_name, "Service");

        // Shared fields appear on every variant.
        assert_eq!(t.shared_fields.len(), 1);
        assert_eq!(t.shared_fields[0].name, "description");
        // The shared optional stays Option<T> — it's genuinely
        // optional within every variant.
        assert!(t.shared_fields[0].optional);
    }

    /// Codegen of a tagged-enum schema emits a Rust `enum` and a
    /// `TryFrom<&ValueRef>` impl that dispatches by discriminator.
    #[test]
    fn tagged_enum_emits_rust_enum_and_dispatch() {
        let src = concat!(
            "# @rust: tagged_enum(discriminator = \"type\")\n",
            "schema TestAssertion:\n",
            "    type: \"command\" | \"service\"\n",
            "    description?: str  # @rust: shared\n",
            "    command?: str  # @rust: variant(\"command\")\n",
            "    service?: str  # @rust: variant(\"service\")\n",
        );
        let out = generate_to_string(src).expect("generate");
        // Enum definition with payload variants
        assert!(
            out.contains("pub enum TestAssertion {"),
            "expected `pub enum TestAssertion`; got:\n{out}"
        );
        assert!(out.contains("Command {"), "expected Command variant");
        assert!(out.contains("Service {"), "expected Service variant");
        // Variant-required fields are non-Option (collapsed from
        // KCL's optional declaration).
        assert!(
            out.contains("command: String,"),
            "command field should be non-optional in Command variant"
        );
        // Shared optional fields stay Option<T>.
        assert!(
            out.contains("description: Option<String>,"),
            "description should be Option<String>"
        );
        // TryFrom reads the discriminator and dispatches.
        assert!(out.contains("impl TryFrom<&kcl_runtime::ValueRef> for TestAssertion"));
        assert!(
            out.contains("\"command\" =>"),
            "TryFrom should match on discriminator literal"
        );
        assert!(out.contains("Ok(TestAssertion::Command {"));
        // No flat struct emission for the tagged-enum schema.
        assert!(
            !out.contains("pub struct TestAssertion {"),
            "tagged-enum schema should NOT emit a flat struct"
        );
        // No separate lifted enum for the discriminator either.
        assert!(
            !out.contains("pub enum TestAssertionType {"),
            "discriminator's lifted enum should be elided"
        );
    }

    /// Missing per-field annotation on a tagged_enum schema errors
    /// loudly — the consumer must be explicit about which variant
    /// each field belongs to.
    #[test]
    fn tagged_enum_missing_field_annotation_errors() {
        let src = concat!(
            "# @rust: tagged_enum(discriminator = \"type\")\n",
            "schema TestAssertion:\n",
            "    type: \"command\" | \"service\"\n",
            "    command?: str  # @rust: variant(\"command\")\n",
            // service field intentionally has NO annotation.
            "    service?: str\n",
        );
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::InvalidAnnotation(msg) => {
                assert!(
                    msg.contains("service") && msg.contains("variant"),
                    "msg should name the offending field; got: {msg}"
                );
            }
            other => panic!("expected InvalidAnnotation, got: {other:?}"),
        }
    }

    /// variant("X") naming a non-existent discriminator literal
    /// errors with a list of valid variant names.
    #[test]
    fn tagged_enum_variant_name_mismatch_errors() {
        let src = concat!(
            "# @rust: tagged_enum(discriminator = \"type\")\n",
            "schema TestAssertion:\n",
            "    type: \"command\" | \"service\"\n",
            // Typo: "command" misspelled.
            "    command?: str  # @rust: variant(\"commandt\")\n",
            "    service?: str  # @rust: variant(\"service\")\n",
        );
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::InvalidAnnotation(msg) => {
                assert!(
                    msg.contains("commandt") && msg.contains("does not match"),
                    "msg should name the offending variant; got: {msg}"
                );
            }
            other => panic!("expected InvalidAnnotation, got: {other:?}"),
        }
    }

    /// tagged_enum naming a discriminator field that isn't a
    /// string-literal union errors with an actionable message.
    #[test]
    fn tagged_enum_wrong_discriminator_kind_errors() {
        let src = concat!(
            "# @rust: tagged_enum(discriminator = \"type\")\n",
            "schema X:\n",
            "    type: int\n",
            "    a: str  # @rust: variant(\"command\")\n",
        );
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::InvalidAnnotation(msg) => {
                assert!(
                    msg.contains("must be a string-literal"),
                    "msg should explain the constraint; got: {msg}"
                );
            }
            other => panic!("expected InvalidAnnotation, got: {other:?}"),
        }
    }

    /// A schema with no `@rust:` annotations at all should leave
    /// `annotations` empty — codegen continues to use the default
    /// flat-struct path.
    #[test]
    fn no_annotations_means_empty_annotations_field() {
        let src = concat!("schema Plain:\n", "    name: str\n", "    count: int = 1\n",);
        let module = analyse_inline_source(src).expect("analyse");
        let s = &module.schemas[0];
        assert!(s.annotations.tagged_enum.is_none());
        assert!(s.annotations.fields.is_empty());
    }

    /// Malformed annotations surface as `CodegenError::InvalidAnnotation`
    /// rather than a confusing parse/resolve error or silently
    /// ignored. Locks down the "fail loudly" contract.
    #[test]
    fn malformed_annotation_errors_with_invalid_annotation() {
        let src = concat!(
            "# @rust: tagged_enum(missing_paren_close\n",
            "schema X:\n",
            "    a: str\n",
        );
        let err = analyse_inline_source(src).expect_err("should fail");
        assert!(
            matches!(err, CodegenError::InvalidAnnotation(_)),
            "expected InvalidAnnotation; got {err:?}"
        );
    }

    /// Lifted enum name collision: a schema literally named the same
    /// as a discriminator's PascalCase concat would emit two `pub`
    /// items with the same identifier. Codegen detects this and
    /// errors before emission rather than producing source that
    /// fails to compile in confusing ways.
    #[test]
    fn lifted_enum_name_collision_is_detected() {
        let src = concat!(
            // A schema literally named VmState — also the
            // PascalCase concat of "Vm" + "state" — would collide
            // with the enum lifted from Vm.state's union below.
            "schema VmState:\n",
            "    label: str\n",
            "\n",
            "schema Vm:\n",
            "    state: \"running\" | \"stopped\"\n",
        );
        let err = generate_to_string(src).expect_err("should fail");
        match err {
            CodegenError::Internal(msg) => {
                assert!(
                    msg.contains("VmState") && msg.contains("collide"),
                    "expected collision diagnostic; got: {msg}"
                );
            }
            other => panic!("expected Internal/collision, got: {other:?}"),
        }
    }
}
