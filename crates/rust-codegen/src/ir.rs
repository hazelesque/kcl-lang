//! Intermediate representation for KCL schemas being codegen'd.
//!
//! [`SchemaIR`] / [`FieldIR`] / [`FieldKind`] are the data structures
//! the emitter walks. They're derived from `kcl-sema`'s resolved
//! `SchemaType` + `SchemaAttr` representation but stripped down to
//! the subset codegen actually consumes — fields, optionality, kind,
//! plus enough source-location info to cite the schema in generated
//! rustdoc.
//!
//! Phase 4 step 3 lands the full IR construction. Step 2 (this commit)
//! ships the data model + a minimal extraction that handles primitive
//! fields only. Subsequent steps fill in lists, dicts, unions,
//! discriminator annotations, etc.

use kcl_ast::ast::Program;
use kcl_sema::resolver::scope::ProgramScope;
use kcl_sema::ty::{SchemaType, TypeKind};

use crate::CodegenError;

/// Codegen IR for a single KCL schema. Produced by [`extract_schemas`]
/// from the resolved program; consumed by `emit_rust_source` in
/// `lib.rs`.
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
}

/// Codegen IR for a single field within a [`SchemaIR`].
#[derive(Debug, Clone)]
pub struct FieldIR {
    /// Field name. Reused verbatim as the generated Rust field name.
    pub name: String,
    /// Field type kind. See [`FieldKind`] for the supported subset.
    pub kind: FieldKind,
    /// `field?: T` declaration — codegen wraps in `Option<T>` and
    /// maps `Value::undefined` to `None` per D5.
    pub optional: bool,
    /// `field: T = expr` declaration — KCL's VM populates the default
    /// during evaluation, so codegen does not emit a `Default` impl
    /// (per the plan's Phase 4 non-scope). Stored for rustdoc.
    pub has_default: bool,
}

/// The Rust-type-shape a [`FieldIR`] codegens to. Phase 4 step 2
/// ships primitive kinds; subsequent steps extend with list, dict,
/// nested-schema, and union kinds.
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
    /// A kind the Phase 4 MVP doesn't yet handle. Carries a
    /// descriptive label so codegen errors point at the actual
    /// unsupported shape rather than `Unknown`.
    Unsupported(String),
}

/// Walk the resolved program's main package, extract every schema
/// declaration as a [`SchemaIR`].
pub(crate) fn extract_schemas(
    program: &Program,
    scope: &ProgramScope,
) -> Result<Vec<SchemaIR>, CodegenError> {
    let mut out = Vec::new();
    // The main package's resolved scope holds the schemas declared
    // in the input. Walk its variables looking for SchemaType values
    // (sema represents `schema Foo: ...` declarations as a variable
    // of TypeKind::Schema in the package's scope).
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
        None => {
            // No main package — empty input, nothing to do.
            return Ok(out);
        }
    };

    let scope_borrow = main_pkg_scopes.borrow();
    for (name, obj_rc) in scope_borrow.elems.iter() {
        let obj = obj_rc.borrow();
        let ty = match &obj.ty.kind {
            TypeKind::Schema(schema_ty) => schema_ty.clone(),
            _ => continue,
        };
        // Phase 4 step 2: skip mixins and protocols. Inheritance and
        // mixin-as-trait codegen are deferred (D9/D10 per the plan).
        // Mixins are transparent for the consuming schema, so the
        // schema using them codegens normally; we just don't emit a
        // standalone type for the mixin itself.
        if ty.is_mixin || ty.is_protocol {
            continue;
        }
        out.push(schema_to_ir(name.clone(), &ty)?);
    }
    Ok(out)
}

fn schema_to_ir(name: String, ty: &SchemaType) -> Result<SchemaIR, CodegenError> {
    if ty.base.is_some() {
        return Err(CodegenError::UnsupportedFeature {
            feature: "schema_inheritance",
            location: format!("schema {} ({}:{})", name, ty.filename, schema_line(ty)),
        });
    }

    let mut fields = Vec::with_capacity(ty.attrs.len());
    for (field_name, attr) in ty.attrs.iter() {
        let kind = field_kind_for(&attr.ty.kind);
        fields.push(FieldIR {
            name: field_name.clone(),
            kind,
            optional: attr.is_optional,
            has_default: attr.has_default,
        });
    }

    Ok(SchemaIR {
        name,
        source_file: ty.filename.clone(),
        source_line: schema_line(ty),
        fields,
        doc: ty.doc.clone(),
    })
}

fn schema_line(ty: &SchemaType) -> u64 {
    // SchemaType doesn't carry its own range directly in this view;
    // fields do. For step 2 we surface line 0 as a placeholder; step
    // 3 will plumb the declaration site through (sema records it on
    // the schema's defining AST node).
    let _ = ty;
    0
}

fn field_kind_for(ty_kind: &TypeKind) -> FieldKind {
    match ty_kind {
        TypeKind::Int | TypeKind::IntLit(_) => FieldKind::Int,
        TypeKind::Float | TypeKind::FloatLit(_) => FieldKind::Float,
        TypeKind::Bool | TypeKind::BoolLit(_) => FieldKind::Bool,
        TypeKind::Str | TypeKind::StrLit(_) => FieldKind::Str,
        other => FieldKind::Unsupported(format!("{other:?}")),
    }
}
