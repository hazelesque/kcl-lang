//! Intermediate representation for KCL schemas being codegen'd.
//!
//! [`ModuleIR`] is the top-level data structure the emitter walks.
//! It holds the schemas in declaration order plus any string-literal
//! unions ([`EnumIR`]) lifted out as named Rust enums.
//!
//! [`SchemaIR`] / [`FieldIR`] / [`FieldKind`] / [`EnumIR`] are
//! derived from `kcl-sema`'s resolved `SchemaType` + `SchemaAttr`
//! representation, stripped down to the subset codegen consumes.

use std::collections::HashMap;

use kcl_ast::ast::{Program, SchemaAttr, SchemaStmt, Stmt};
use kcl_sema::resolver::scope::ProgramScope;
use kcl_sema::ty::{SchemaType, TypeKind};

use crate::CodegenError;
use crate::annotation::{FieldAnnotation, RawAnnotation, SchemaAnnotations, parse_rust_annotation};

/// Top-level codegen IR for a single KCL source file.
#[derive(Debug, Clone, Default)]
pub struct ModuleIR {
    /// Schema definitions in declaration order.
    pub schemas: Vec<SchemaIR>,
    /// String-literal-union enums lifted out for naming. Each
    /// occurrence of e.g. `field: "a" | "b" | "c"` produces one
    /// entry here, with a name derived from `<SchemaName><FieldName>`
    /// (PascalCase concat).
    pub enums: Vec<EnumIR>,
    /// Tagged-enum schemas lifted from `schemas` via the Phase 4B
    /// `# @rust: tagged_enum(...)` annotation. Distinct from `enums`
    /// (which holds string-literal-union lifts); these carry
    /// per-variant field lists and codegen as a Rust enum with
    /// payload variants rather than C-style unit variants.
    pub tagged_enums: Vec<TaggedEnumIR>,
}

/// Codegen IR for a schema that's been promoted to a tagged Rust
/// enum via the `# @rust: tagged_enum(discriminator = "...")`
/// annotation. The discriminator field's string-literal-union values
/// become the enum's variants; each non-discriminator field is
/// assigned to a variant (`# @rust: variant("X")`) or appears in
/// every variant (`# @rust: shared`).
#[derive(Debug, Clone)]
pub struct TaggedEnumIR {
    /// Generated Rust type name — the schema's name verbatim.
    pub name: String,
    /// KCL source file the schema was declared in. Surfaces in
    /// rustdoc.
    pub source_file: String,
    /// Source line of the `schema X:` declaration.
    pub source_line: u64,
    /// The schema's docstring, forwarded to the generated enum's
    /// top-level rustdoc.
    pub doc: String,
    /// Name of the discriminator field (the field whose
    /// string-literal-union values enumerate the variants). The
    /// field itself is *not* a member of any variant — its value
    /// determines which variant is constructed.
    pub discriminator: String,
    /// One entry per discriminator literal, in source order.
    pub variants: Vec<TaggedEnumVariantIR>,
    /// Fields that appear in every variant (annotated
    /// `# @rust: shared`). Their `Option`-ness and default-ness
    /// behave the same as in struct codegen.
    pub shared_fields: Vec<FieldIR>,
}

/// One variant of a [`TaggedEnumIR`].
#[derive(Debug, Clone)]
pub struct TaggedEnumVariantIR {
    /// PascalCased Rust variant name (e.g. `"command"` → `Command`).
    pub rust_name: String,
    /// The discriminator literal as it appears in KCL source.
    pub kcl_literal: String,
    /// Fields assigned to this variant via `# @rust: variant("X")`.
    /// All have their `optional` collapsed to `false` (within an
    /// assigned variant, the KCL check block ensures the field is
    /// present, so codegen emits it as a non-`Option<T>`). Fields
    /// with `has_default` stay as `T` (default flows from VM).
    pub fields: Vec<FieldIR>,
}

/// Codegen IR for a single KCL schema. Produced by [`extract_schemas`]
/// from the resolved program; consumed by the emitter.
#[derive(Debug, Clone)]
pub struct SchemaIR {
    /// The schema's KCL name. Reused verbatim as the generated Rust
    /// type name.
    pub name: String,
    /// Source file where the schema was declared. Surfaces in
    /// generated rustdoc per the scm-infra "cite-the-source"
    /// convention.
    pub source_file: String,
    /// Source line of the schema declaration. Surfaces in rustdoc.
    pub source_line: u64,
    /// Fields in declaration order. KCL preserves order via sema's
    /// `IndexMap`; consumers rely on it for ergonomic struct layout.
    pub fields: Vec<FieldIR>,
    /// The schema's docstring, if any. Forwarded to the generated
    /// Rust type's rustdoc.
    pub doc: String,
    /// `# @rust:` annotations parsed from the comments surrounding
    /// this schema's declaration. `None` means the schema codegens
    /// as the default flat struct (the Phase 4 MVP shape); `Some(_)`
    /// with a `tagged_enum` annotation means [`crate::emit`] should
    /// promote the schema to a Rust `enum`. See
    /// [`crate::annotation`] for the annotation grammar.
    pub(crate) annotations: SchemaAnnotations,
}

/// Codegen IR for a single field within a [`SchemaIR`].
#[derive(Debug, Clone)]
pub struct FieldIR {
    /// Field name as it appears in KCL source. Used verbatim as the
    /// generated Rust field name *after* keyword escaping
    /// (`type` → `r#type`, etc.).
    pub name: String,
    /// Field type kind. See [`FieldKind`] for the supported subset.
    pub kind: FieldKind,
    /// `field?: T` declaration — codegen wraps in `Option<T>` and
    /// maps `Value::undefined`/`Value::none` to `None` per D5.
    pub optional: bool,
    /// `field: T = expr` declaration — KCL's VM populates the default
    /// during evaluation, so codegen does not emit a `Default` impl
    /// (per the plan's Phase 4 non-scope). Stored for rustdoc.
    pub has_default: bool,
}

/// Codegen IR for a string-literal union, lifted to a named Rust enum.
#[derive(Debug, Clone)]
pub struct EnumIR {
    /// Generated Rust type name (PascalCase). Unique within the
    /// emitted module.
    pub rust_name: String,
    /// Variant strings as they appear in the KCL source. Order
    /// preserved; the generated Rust enum has one variant per entry,
    /// named via [`pascal_case`].
    pub variants: Vec<String>,
    /// The schema + field this union was lifted from. Surfaces in
    /// the generated enum's rustdoc.
    pub origin: String,
}

/// The Rust-type-shape a [`FieldIR`] codegens to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// `int` (i64)
    Int,
    /// `float` (f64)
    Float,
    /// `bool`
    Bool,
    /// `str` (String)
    Str,
    /// `[T]` — list of the inner kind.
    List(Box<FieldKind>),
    /// `{K:V}` — dict with the given key and value kinds.
    Dict(Box<FieldKind>, Box<FieldKind>),
    /// Reference to another schema by name. Codegen emits the field
    /// as the other schema's generated Rust type and delegates to
    /// that type's `TryFrom<&ValueRef>` in the impl.
    Schema(String),
    /// Reference to a string-literal union enum (lifted to
    /// [`ModuleIR::enums`]) by its generated Rust name. The variants
    /// themselves are stored on the [`EnumIR`].
    StrEnum(String),
    /// F1.6: mokkan native typed inet — emits as `cidr::IpCidr`.
    /// Consumer crate must depend on `cidr = "0.3"` (same version
    /// as the fork) so the round-trip type matches.
    Cidr,
    /// F1.6: mokkan native typed inet — emits as `cidr::IpInet`.
    Inet,
    /// F1.6: mokkan native MAC — emits as `macaddr::MacAddr6`.
    Macaddr,
    /// F1.6: mokkan native MAC-8 — emits as `macaddr::MacAddr8`.
    Macaddr8,
    /// F1.6: mokkan native IP family enum — emits as
    /// `kcl_runtime::IpFamily`. (Stable location across the
    /// post-F2 `kcl_*` → `mokkan_*` crate rename; regenerated code
    /// picks up the new path at re-codegen time.)
    IpFamily,
    /// F2.7b: `T | ResolvableString` union shape. Codegen emits as
    /// `kcl_embed::resolve::Resolvable<T>` and the generated
    /// TryFrom dispatches on `is_resolvable_string` to pick the
    /// Pending arm (segment list preserved) vs. the Resolved arm
    /// (inner FieldKind's TryFrom). Box keeps the FieldKind enum's
    /// stack footprint manageable; the inner kind is the T from
    /// the union.
    Resolvable(Box<FieldKind>),
    /// A kind the Phase 4 MVP doesn't yet handle. Carries a
    /// descriptive label so codegen errors point at the actual
    /// unsupported shape rather than `Unknown`.
    Unsupported(String),
}

impl FieldKind {
    /// Rust type expression for this kind. Used in generated struct
    /// fields and helper invocations.
    pub fn to_rust_type(&self) -> String {
        match self {
            FieldKind::Int => "i64".to_string(),
            FieldKind::Float => "f64".to_string(),
            FieldKind::Bool => "bool".to_string(),
            FieldKind::Str => "String".to_string(),
            FieldKind::List(inner) => format!("Vec<{}>", inner.to_rust_type()),
            FieldKind::Dict(k, v) => format!(
                "std::collections::HashMap<{}, {}>",
                k.to_rust_type(),
                v.to_rust_type()
            ),
            FieldKind::Schema(name) => name.clone(),
            FieldKind::StrEnum(name) => name.clone(),
            FieldKind::Cidr => "cidr::IpCidr".to_string(),
            FieldKind::Inet => "cidr::IpInet".to_string(),
            FieldKind::Macaddr => "macaddr::MacAddr6".to_string(),
            FieldKind::Macaddr8 => "macaddr::MacAddr8".to_string(),
            FieldKind::IpFamily => "kcl_runtime::IpFamily".to_string(),
            FieldKind::Resolvable(inner) => {
                format!("kcl_embed::resolve::Resolvable<{}>", inner.to_rust_type())
            }
            FieldKind::Unsupported(label) => format!("/* UNSUPPORTED: {label} */ ()"),
        }
    }

    /// Whether this kind is the Unsupported sentinel — emission
    /// checks this to bail with a CodegenError rather than producing
    /// an unbuildable `/* UNSUPPORTED */ ()` field type.
    pub fn is_unsupported(&self) -> bool {
        matches!(self, FieldKind::Unsupported(_))
    }

    /// Recursively check whether any nested kind is Unsupported.
    pub fn has_unsupported(&self) -> bool {
        match self {
            FieldKind::Unsupported(_) => true,
            FieldKind::List(inner) => inner.has_unsupported(),
            FieldKind::Dict(k, v) => k.has_unsupported() || v.has_unsupported(),
            FieldKind::Resolvable(inner) => inner.has_unsupported(),
            _ => false,
        }
    }

    /// Best-effort description of this kind for error messages.
    pub fn describe(&self) -> String {
        match self {
            FieldKind::Unsupported(label) => label.clone(),
            other => other.to_rust_type(),
        }
    }

    /// Recursively check whether this kind transitively contains a
    /// [`FieldKind::Float`]. Used by the emitter to surface the
    /// NaN-aware-PartialEq smell on generated structs that derive
    /// `PartialEq` and have an `f64` field. (Phase 4 MVP keeps the
    /// blanket derive; future work may switch to per-schema
    /// conditional derives or a custom-impl shape.)
    pub fn contains_float(&self) -> bool {
        match self {
            FieldKind::Float => true,
            FieldKind::List(inner) => inner.contains_float(),
            FieldKind::Dict(k, v) => k.contains_float() || v.contains_float(),
            FieldKind::Resolvable(inner) => inner.contains_float(),
            _ => false,
        }
    }
}

/// Walk the resolved program's main package, extract every schema
/// declaration as a [`SchemaIR`], collect any lifted enums into the
/// shared [`ModuleIR`]. Parses `# @rust:` annotations from the AST
/// comments and attaches them to the appropriate schema/field IR.
pub(crate) fn extract_module(
    program: &Program,
    scope: &ProgramScope,
) -> Result<ModuleIR, CodegenError> {
    let mut module = ModuleIR::default();
    let main_pkg_scopes = match program.pkgs.get(kcl_ast::MAIN_PKG) {
        Some(_) => match scope.scope_map.get(kcl_ast::MAIN_PKG) {
            Some(s) => s,
            None => {
                return Err(CodegenError::Internal(format!(
                    "main package {:?} has no resolved scope",
                    kcl_ast::MAIN_PKG
                )));
            }
        },
        None => return Ok(module),
    };

    // Build a name → (line → AST positions) index from the program's
    // modules. We use it to look up per-schema and per-field source
    // positions for annotation matching. (kcl-sema's `SchemaType`
    // does carry `filename` and the scope's start.line, but not
    // per-attribute lines.)
    let position_index = build_position_index(program);

    let scope_borrow = main_pkg_scopes.borrow();
    for (name, obj_rc) in scope_borrow.elems.iter() {
        let obj = obj_rc.borrow();
        let ty = match &obj.ty.kind {
            TypeKind::Schema(schema_ty) => schema_ty.clone(),
            _ => continue,
        };
        if ty.is_mixin || ty.is_protocol {
            continue;
        }
        let source_line = obj.start.line;
        let positions = position_index.get(name.as_str());
        let schema_ir = schema_to_ir(name.clone(), &ty, source_line, positions, &mut module)?;
        module.schemas.push(schema_ir);
    }

    // Phase 4B promotion: schemas annotated with `tagged_enum` get
    // moved out of `module.schemas` into `module.tagged_enums` with
    // their fields rebucketed by variant. Done after all schemas
    // have been extracted so the lifted-enum-for-the-discriminator
    // can also be cleaned up from `module.enums`.
    promote_tagged_enum_schemas(&mut module)?;

    detect_lifted_enum_collisions(&module)?;

    Ok(module)
}

/// Walk `module.schemas`, lift any with a `tagged_enum` annotation
/// into `module.tagged_enums`, and remove the now-redundant lifted
/// string-literal-union for the discriminator field from
/// `module.enums`.
///
/// Validation:
/// - The named discriminator field must exist on the schema.
/// - It must be a [`FieldKind::StrEnum`] (string-literal union).
/// - Every non-discriminator field must have either `variant(X)` or
///   `shared` annotation.
/// - Every `variant("X")` name must refer to a declared discriminator
///   literal.
fn promote_tagged_enum_schemas(module: &mut ModuleIR) -> Result<(), CodegenError> {
    // Indices in module.schemas that should be lifted, in reverse
    // order so the swap_remove inside the loop doesn't perturb the
    // indices we haven't visited yet.
    let to_lift: Vec<usize> = module
        .schemas
        .iter()
        .enumerate()
        .filter(|(_, s)| s.annotations.tagged_enum.is_some())
        .map(|(i, _)| i)
        .collect();

    // Lift in reverse so swap_remove indices stay valid.
    for idx in to_lift.into_iter().rev() {
        let schema = module.schemas.swap_remove(idx);
        let tagged = lift_to_tagged_enum(schema, module)?;
        module.tagged_enums.push(tagged);
    }

    // Re-sort schemas back into source-declaration order: swap_remove
    // doesn't preserve order, but the rest of the codegen (and
    // generated rustdoc citing source lines) reads better when the
    // emission order matches source order.
    module.schemas.sort_by_key(|s| s.source_line);
    // Same for tagged enums.
    module.tagged_enums.sort_by_key(|t| t.source_line);

    Ok(())
}

/// Convert one [`SchemaIR`] (with a tagged_enum annotation) into a
/// [`TaggedEnumIR`]. Removes the lifted enum that was created for
/// the discriminator field from `module.enums` since that
/// information is now embedded in the tagged enum's variant set.
fn lift_to_tagged_enum(
    schema: SchemaIR,
    module: &mut ModuleIR,
) -> Result<TaggedEnumIR, CodegenError> {
    let discriminator = schema
        .annotations
        .tagged_enum
        .as_ref()
        .expect("promotion only invoked when tagged_enum annotation present")
        .discriminator
        .clone();

    // Find the discriminator field by name. It must be a lifted
    // string-literal-union (StrEnum) so the variants are known.
    let disc_field = schema
        .fields
        .iter()
        .find(|f| f.name == discriminator)
        .ok_or_else(|| {
            CodegenError::InvalidAnnotation(format!(
                "schema {}: tagged_enum discriminator `{}` is not a field",
                schema.name, discriminator,
            ))
        })?;
    let disc_enum_name = match &disc_field.kind {
        FieldKind::StrEnum(name) => name.clone(),
        other => {
            return Err(CodegenError::InvalidAnnotation(format!(
                "schema {}: tagged_enum discriminator `{}` must be a string-literal \
                 union (e.g. `\"a\" | \"b\"`), got {}",
                schema.name,
                discriminator,
                other.describe(),
            )));
        }
    };

    // Pull the variant literals out of the EnumIR. We *clone* before
    // any removal so the variant list is available even after the
    // EnumIR is removed from module.enums.
    let lifted_idx = module
        .enums
        .iter()
        .position(|e| e.rust_name == disc_enum_name)
        .ok_or_else(|| {
            CodegenError::Internal(format!(
                "schema {}: discriminator field had StrEnum kind referring to `{}` \
                 but no matching lifted enum was found in module.enums",
                schema.name, disc_enum_name,
            ))
        })?;
    let lifted_variants = module.enums[lifted_idx].variants.clone();

    // Bucket non-discriminator fields into per-variant lists and a
    // shared list. Reject fields without annotation. Validate every
    // variant("X") name matches a declared literal.
    let mut variant_fields: Vec<(String, Vec<FieldIR>)> = lifted_variants
        .iter()
        .map(|v| (v.clone(), Vec::new()))
        .collect();
    let mut shared_fields = Vec::new();

    for field in &schema.fields {
        if field.name == discriminator {
            continue;
        }
        let anno = schema.annotations.fields.get(&field.name).ok_or_else(|| {
            CodegenError::InvalidAnnotation(format!(
                "schema {}.{}: tagged_enum requires every non-discriminator field to \
                 carry either `# @rust: variant(\"<name>\")` or `# @rust: shared`",
                schema.name, field.name,
            ))
        })?;
        match anno {
            FieldAnnotation::Shared => {
                shared_fields.push(field.clone());
            }
            FieldAnnotation::Variant {
                name: variant_name,
                is_variant_optional,
            } => {
                let bucket = variant_fields
                    .iter_mut()
                    .find(|(v, _)| v == variant_name)
                    .ok_or_else(|| {
                        CodegenError::InvalidAnnotation(format!(
                            "schema {}.{}: variant(\"{}\") does not match any \
                             discriminator literal; declared variants are {:?}",
                            schema.name, field.name, variant_name, lifted_variants,
                        ))
                    })?;
                // Default: within an assigned variant the field is
                // required (the schema's check: block enforces this),
                // so collapse `?:` → non-Option. The
                // `variant("X", optional)` form opts out of the
                // collapse: keep Option<T> when the field is
                // genuinely optional within the variant (e.g. the
                // contains? field on the File variant of Tilley's
                // TestAssertion). Defaulted fields (has_default) stay
                // as `T` regardless — the VM populates the default.
                let mut variant_field = field.clone();
                if !*is_variant_optional {
                    variant_field.optional = false;
                }
                bucket.1.push(variant_field);
            }
        }
    }

    // Build the TaggedEnumVariantIR list, preserving the declaration
    // order of the discriminator literals.
    let variants: Vec<TaggedEnumVariantIR> = variant_fields
        .into_iter()
        .map(|(kcl_literal, fields)| TaggedEnumVariantIR {
            rust_name: pascal_case(&kcl_literal),
            kcl_literal,
            fields,
        })
        .collect();

    // Drop the now-redundant lifted enum for the discriminator field.
    // After lifting, the variant set is encoded in the TaggedEnumIR
    // itself; emitting a separate `pub enum TestAssertionType { ... }`
    // would pollute the namespace and confuse consumers about which
    // is "the" tagged-enum type.
    module.enums.swap_remove(lifted_idx);

    Ok(TaggedEnumIR {
        name: schema.name,
        source_file: schema.source_file,
        source_line: schema.source_line,
        doc: schema.doc,
        discriminator,
        variants,
        shared_fields,
    })
}

/// Per-schema source positions captured from the AST, indexed by
/// field name. The schema's own declaration line and the module's
/// comment list ride alongside.
#[derive(Debug, Default)]
struct SchemaPositions {
    /// Line of the `schema X:` declaration.
    schema_line: u64,
    /// Field name → declaration line.
    field_lines: HashMap<String, u64>,
    /// The module's comment list, cloned so it can be matched to
    /// schemas across iterations without re-locking the parser's
    /// shared module storage.
    comments: Vec<CommentPos>,
}

/// Minimal projection of an AST `Comment` node — text + line — so
/// the annotation parser can match by line number without depending
/// on the full AST representation.
#[derive(Debug, Clone)]
struct CommentPos {
    text: String,
    line: u64,
}

/// Build the per-schema position index from `program.modules`.
fn build_position_index(program: &Program) -> HashMap<String, SchemaPositions> {
    let mut out: HashMap<String, SchemaPositions> = HashMap::new();

    for module_arc in program.modules.values() {
        let Ok(module) = module_arc.read() else {
            continue;
        };
        let comments: Vec<CommentPos> = module
            .comments
            .iter()
            .map(|c| CommentPos {
                text: c.node.text.clone(),
                line: c.line,
            })
            .collect();
        for stmt_node in &module.body {
            if let Stmt::Schema(schema_stmt) = &stmt_node.node {
                let positions = build_positions_for_schema(stmt_node.line, schema_stmt, &comments);
                out.insert(schema_stmt.name.node.clone(), positions);
            }
        }
    }

    out
}

fn build_positions_for_schema(
    schema_line: u64,
    schema_stmt: &SchemaStmt,
    comments: &[CommentPos],
) -> SchemaPositions {
    let mut field_lines = HashMap::new();
    for body_node in &schema_stmt.body {
        if let Stmt::SchemaAttr(SchemaAttr { name, .. }) = &body_node.node {
            field_lines.insert(name.node.clone(), body_node.line);
        }
    }
    SchemaPositions {
        schema_line,
        field_lines,
        comments: comments.to_vec(),
    }
}

/// A lifted enum's Rust name is `<SchemaName><FieldName>` in
/// PascalCase. If the user happens to define a schema literally
/// named (say) `VmState` alongside a `Vm.state` discriminator, the
/// emitted file would contain two `pub` items called `VmState` and
/// fail to compile. Detect that early with a clear error rather
/// than letting rustc surface a confused message about duplicate
/// definitions in generated code.
fn detect_lifted_enum_collisions(module: &ModuleIR) -> Result<(), CodegenError> {
    use std::collections::HashSet;
    let schema_names: HashSet<&str> = module.schemas.iter().map(|s| s.name.as_str()).collect();
    for e in &module.enums {
        if schema_names.contains(e.rust_name.as_str()) {
            return Err(CodegenError::Internal(format!(
                "lifted enum `{}` (from {}) collides with a schema of the same name; \
                 rename the schema or the discriminator field to disambiguate",
                e.rust_name, e.origin,
            )));
        }
    }
    let mut enum_names: HashSet<&str> = HashSet::new();
    for e in &module.enums {
        if !enum_names.insert(e.rust_name.as_str()) {
            return Err(CodegenError::Internal(format!(
                "lifted enum name `{}` is duplicated (origins include {}); \
                 two fields with the same parent + field name pair produced a \
                 collision",
                e.rust_name, e.origin,
            )));
        }
    }
    Ok(())
}

fn schema_to_ir(
    name: String,
    ty: &SchemaType,
    source_line: u64,
    positions: Option<&SchemaPositions>,
    module: &mut ModuleIR,
) -> Result<SchemaIR, CodegenError> {
    if ty.base.is_some() {
        return Err(CodegenError::UnsupportedFeature {
            feature: "schema_inheritance",
            location: format!("schema {} ({})", name, ty.filename),
        });
    }

    let mut fields = Vec::with_capacity(ty.attrs.len());
    for (field_name, attr) in ty.attrs.iter() {
        let kind = field_kind_for(&attr.ty.kind, &name, field_name, module);
        if kind.has_unsupported() {
            return Err(CodegenError::UnsupportedFeature {
                feature: "field_kind",
                location: format!(
                    "schema {}.{} ({}): kind {}",
                    name,
                    field_name,
                    ty.filename,
                    kind.describe()
                ),
            });
        }
        fields.push(FieldIR {
            name: field_name.clone(),
            kind,
            optional: attr.is_optional,
            has_default: attr.has_default,
        });
    }

    let annotations = match positions {
        Some(p) => extract_annotations(&name, p)?,
        None => SchemaAnnotations::default(),
    };

    Ok(SchemaIR {
        name,
        source_file: ty.filename.clone(),
        source_line,
        fields,
        doc: ty.doc.clone(),
        annotations,
    })
}

/// Match comments against the schema's declaration position and each
/// of its fields' positions, parse any `@rust:` annotations into the
/// [`SchemaAnnotations`] structure.
///
/// Matching rules:
/// - **Schema-level annotation** lives on the *immediately preceding*
///   line (e.g. `# @rust: tagged_enum(...)` on line N-1 when the
///   schema is declared at line N).
/// - **Field-level annotation** lives as a *trailing* comment on the
///   same line as the field declaration (e.g.
///   `command?: str  # @rust: variant("command")` on the same line).
///   This matches the KCL convention where comments after a
///   declaration's `:` are commonly used for inline notes.
///
/// The annotation parser surfaces malformed `@rust:` directives as
/// `CodegenError::InvalidAnnotation`; this function propagates those
/// without modification.
fn extract_annotations(
    schema_name: &str,
    positions: &SchemaPositions,
) -> Result<SchemaAnnotations, CodegenError> {
    let mut out = SchemaAnnotations::default();

    // Schema-level: comment on the line immediately above the
    // `schema X:` declaration.
    if positions.schema_line > 0 {
        let target_line = positions.schema_line - 1;
        for c in &positions.comments {
            if c.line == target_line {
                if let Some(anno) = parse_rust_annotation(&c.text)? {
                    match anno {
                        RawAnnotation::TaggedEnum(t) => {
                            if out.tagged_enum.is_some() {
                                return Err(CodegenError::InvalidAnnotation(format!(
                                    "schema {schema_name}: duplicate tagged_enum annotation"
                                )));
                            }
                            out.tagged_enum = Some(t);
                        }
                        RawAnnotation::Field(_) => {
                            return Err(CodegenError::InvalidAnnotation(format!(
                                "schema {schema_name}: field-level annotation on schema declaration line"
                            )));
                        }
                    }
                }
            }
        }
    }

    // Field-level: trailing comments on the same line as the field
    // declaration. The comments preserved on `Module.comments`
    // include a leading `#` but no further structural info, so the
    // "trailing same line" check is line-equality.
    for (field_name, field_line) in &positions.field_lines {
        for c in &positions.comments {
            if c.line != *field_line {
                continue;
            }
            let Some(anno) = parse_rust_annotation(&c.text)? else {
                continue;
            };
            match anno {
                RawAnnotation::Field(f) => {
                    if out.fields.contains_key(field_name) {
                        return Err(CodegenError::InvalidAnnotation(format!(
                            "schema {schema_name}.{field_name}: duplicate field annotation"
                        )));
                    }
                    out.fields.insert(field_name.clone(), f);
                }
                RawAnnotation::TaggedEnum(_) => {
                    return Err(CodegenError::InvalidAnnotation(format!(
                        "schema {schema_name}.{field_name}: schema-level annotation on field line"
                    )));
                }
            }
        }
    }

    Ok(out)
}

/// Map a `kcl_sema::ty::TypeKind` to a [`FieldKind`]. Side-effects:
/// if the type is a string-literal union, an [`EnumIR`] is added to
/// `module.enums` and the returned `FieldKind::StrEnum` references it
/// by name.
fn field_kind_for(
    ty_kind: &TypeKind,
    schema_name: &str,
    field_name: &str,
    module: &mut ModuleIR,
) -> FieldKind {
    match ty_kind {
        TypeKind::Int | TypeKind::IntLit(_) => FieldKind::Int,
        TypeKind::Float | TypeKind::FloatLit(_) => FieldKind::Float,
        TypeKind::Bool | TypeKind::BoolLit(_) => FieldKind::Bool,
        TypeKind::Str | TypeKind::StrLit(_) => FieldKind::Str,
        TypeKind::List(item_ty) => FieldKind::List(Box::new(field_kind_for(
            &item_ty.kind,
            schema_name,
            field_name,
            module,
        ))),
        TypeKind::Dict(dict_ty) => FieldKind::Dict(
            Box::new(field_kind_for(
                &dict_ty.key_ty.kind,
                schema_name,
                field_name,
                module,
            )),
            Box::new(field_kind_for(
                &dict_ty.val_ty.kind,
                schema_name,
                field_name,
                module,
            )),
        ),
        TypeKind::Schema(schema_ty) => FieldKind::Schema(schema_ty.name.clone()),
        TypeKind::Function(_) => FieldKind::Unsupported(
            "function-typed field (D4 corollary: codegen-time error)".to_string(),
        ),
        TypeKind::Union(members) => {
            // F2.7b: `T | ResolvableString` — the transparent-
            // resolvable shape. Detect first because it takes
            // priority over the string-literal-union path; a
            // schema can't realistically declare
            // `ResolvableString | "lit"` so there's no overlap.
            //
            // Recognition: exactly two arms, exactly one is
            // ResolvableString, the other is an emittable non-
            // union type. Three-or-more-arm unions including
            // ResolvableString (e.g. `int | str | RS`) refuse to
            // codegen — the inner T isn't unambiguous.
            let resolvable_count = members.iter().filter(|m| m.is_resolvable_string()).count();
            if resolvable_count > 0 {
                if members.len() != 2 || resolvable_count != 1 {
                    return FieldKind::Unsupported(format!(
                        "union with ResolvableString must have exactly two arms (one of which \
                         is ResolvableString, the other being the T type). Got {}-arm union: \
                         {}. Three-or-more-arm unions including ResolvableString aren't \
                         supported at F2.7 — split into separate fields, or write the Rust \
                         type by hand.",
                        members.len(),
                        members
                            .iter()
                            .map(|m| m.ty_str())
                            .collect::<Vec<_>>()
                            .join(" | "),
                    ));
                }
                // Find the non-RS arm — that's T.
                let other_ty = members
                    .iter()
                    .find(|m| !m.is_resolvable_string())
                    .expect("resolvable_count == 1 + len == 2 → exactly one non-RS arm exists");
                let inner = field_kind_for(&other_ty.kind, schema_name, field_name, module);
                if inner.is_unsupported() {
                    return FieldKind::Unsupported(format!(
                        "`T | ResolvableString` field where T is itself unsupported: {}",
                        inner.describe()
                    ));
                }
                if matches!(inner, FieldKind::List(_) | FieldKind::Dict(..)) {
                    // The plan permits `[str | ResolvableString]`
                    // (collection of Resolvable<String>) but not
                    // `[str] | ResolvableString` (a collection-or-
                    // string union shape); the latter is ambiguous.
                    // The shape we're rejecting here is the OUTER
                    // union arm being a collection, which the
                    // recognition above would surface as
                    // `Vec<T> | ResolvableString` — not a sensible
                    // operator declaration.
                    return FieldKind::Unsupported(
                        "`T | ResolvableString` where T is a collection (List/Dict): \
                         the operator probably wants `[T | ResolvableString]` (each entry \
                         resolvable) instead of `[T] | ResolvableString` (whole list \
                         OR a deferred string)."
                            .to_string(),
                    );
                }
                return FieldKind::Resolvable(Box::new(inner));
            }
            // String-literal union: `"a" | "b" | "c"`. Codegen lifts
            // this to a named Rust enum at module scope and the field
            // becomes a reference to that enum.
            if let Some(variants) = as_str_literal_union(members) {
                let rust_name = lift_enum_name(schema_name, field_name);
                module.enums.push(EnumIR {
                    rust_name: rust_name.clone(),
                    variants,
                    origin: format!("{schema_name}.{field_name}"),
                });
                FieldKind::StrEnum(rust_name)
            } else {
                FieldKind::Unsupported(format!(
                    "union of non-string-literal types (Phase 4 MVP only supports \
                     string-literal unions like `\"a\" | \"b\"`; for tagged-enum codegen \
                     of overlapping schemas with a discriminator field, add a \
                     `# @rust: tagged_enum(discriminator = \"...\")` annotation — \
                     pending future codegen support — or write the Rust type by hand): \
                     {} | {}",
                    members.first().map(|t| t.ty_str()).unwrap_or_default(),
                    members
                        .get(1)
                        .map(|t| t.ty_str())
                        .unwrap_or_else(|| "...".to_string()),
                ))
            }
        }
        TypeKind::NumberMultiplier(_) => {
            FieldKind::Unsupported("unit-typed value (deferred from Phase 4 MVP)".to_string())
        }
        TypeKind::Any => FieldKind::Unsupported("any-typed field".to_string()),
        TypeKind::None => FieldKind::Unsupported("None-typed field".to_string()),
        TypeKind::Void => FieldKind::Unsupported("Void-typed field".to_string()),
        TypeKind::Module(_) => FieldKind::Unsupported("module-typed field".to_string()),
        TypeKind::Named(name) => FieldKind::Unsupported(format!("Named type alias `{name}`")),
        // F1.6: mokkan native types codegen straight through to the
        // upstream `cidr` / `macaddr` crate types and our own
        // `kcl_runtime::IpFamily` enum. Consumer crates must depend
        // on `cidr = "0.3"` / `macaddr = "1.0"` (workspace-pinned per
        // D7) so the round-trip type matches the runtime accessor's
        // return type.
        TypeKind::Cidr => FieldKind::Cidr,
        TypeKind::Inet => FieldKind::Inet,
        TypeKind::Macaddr => FieldKind::Macaddr,
        TypeKind::Macaddr8 => FieldKind::Macaddr8,
        TypeKind::IpFamily => FieldKind::IpFamily,
        // F2.6: ResolvableString as a standalone field type isn't
        // useful — the operator wants `T | ResolvableString` (the
        // F2.7 "transparent-resolvable" shape that emits as
        // `Resolvable<T>`). A bare-RS field would emit as something
        // like `kcl_runtime::ResolvableString` but consumers can't
        // do anything useful with it because there's no resolver
        // pass to substitute symbolic segments. F2.7 wires the
        // union-shape codegen; until then, refuse with a pointer.
        TypeKind::ResolvableString => FieldKind::Unsupported(
            "bare ResolvableString field — F2.7 wires the `T | ResolvableString` union codegen \
             which emits as `Resolvable<T>`; a bare-RS field has no useful Rust shape until \
             then. If you're declaring a deferred-string slot, write \
             `field: str | ResolvableString` and re-run codegen after F2.7."
                .to_string(),
        ),
    }
}

/// Match `members` against the string-literal-union shape. Returns
/// the variant strings in source order if every member is a
/// `StrLit`, otherwise `None`.
fn as_str_literal_union(members: &[std::sync::Arc<kcl_sema::ty::Type>]) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        match &m.kind {
            TypeKind::StrLit(s) => out.push(s.clone()),
            _ => return None,
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// Build the lifted enum's Rust type name: `<SchemaName><FieldName>`
/// in PascalCase. For Tilley's `TestAssertion.type: "command" | ...`
/// this produces `TestAssertionType`.
fn lift_enum_name(schema_name: &str, field_name: &str) -> String {
    format!("{}{}", schema_name, pascal_case(field_name))
}

/// PascalCase a snake_case / kebab-case / space-separated identifier.
/// `audit_id` -> `AuditId`, `port` -> `Port`, `virtio-serial` ->
/// `VirtioSerial`, `foo bar` -> `FooBar`. Idempotent on already-
/// PascalCase input.
///
/// Treats `_`, `-`, and ` ` as word separators (consumed) and
/// uppercases the next character. Any other non-alphanumeric is
/// also treated as a separator — Rust enum variants need a valid
/// identifier and the input here comes from KCL string-literal
/// unions that the operator wrote, which can legitimately contain
/// any printable character.
pub(crate) fn pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut next_upper = true;
    for c in s.chars() {
        // Word separator: consume, uppercase the next alphanumeric.
        if !c.is_alphanumeric() {
            next_upper = true;
            continue;
        }
        if next_upper {
            for u in c.to_uppercase() {
                out.push(u);
            }
            next_upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Rust reserved words that conflict with KCL field names. The
/// codegen wraps the field name in `r#` when one of these appears so
/// the generated struct is syntactically valid.
pub(crate) fn escape_rust_keyword(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
        "where", "while", "async", "await", "dyn",
    ];
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}
