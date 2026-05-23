//! Intermediate representation for KCL schemas being codegen'd.
//!
//! [`SchemaIR`] / [`FieldIR`] / [`FieldKind`] are the data structures
//! the emitter walks. They're derived from `kcl-sema`'s resolved
//! `SchemaType` + `SchemaAttr` representation but stripped down to
//! the subset codegen actually consumes — fields, optionality, kind,
//! plus enough source-location info to cite the schema in generated
//! rustdoc.

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
    /// maps `Value::undefined`/`Value::none` to `None` per D5.
    pub optional: bool,
    /// `field: T = expr` declaration — KCL's VM populates the default
    /// during evaluation, so codegen does not emit a `Default` impl
    /// (per the plan's Phase 4 non-scope). Stored for rustdoc.
    pub has_default: bool,
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
    /// `[T]` — list of the inner kind. KCL list elements are
    /// homogeneous-typed via sema; the inner kind is whatever
    /// `TypeKind::List`'s element type resolved to.
    List(Box<FieldKind>),
    /// `{K:V}` — dict with the given key and value kinds.
    Dict(Box<FieldKind>, Box<FieldKind>),
    /// Reference to another schema by name. Codegen emits the field
    /// as the other schema's generated Rust type and delegates to
    /// that type's `TryFrom<&ValueRef>` in the impl.
    Schema(String),
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
    /// `Vec<Unsupported>` should fail emission just like
    /// `Unsupported` itself.
    pub fn has_unsupported(&self) -> bool {
        match self {
            FieldKind::Unsupported(_) => true,
            FieldKind::List(inner) => inner.has_unsupported(),
            FieldKind::Dict(k, v) => k.has_unsupported() || v.has_unsupported(),
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
}

/// Walk the resolved program's main package, extract every schema
/// declaration as a [`SchemaIR`].
pub(crate) fn extract_schemas(
    program: &Program,
    scope: &ProgramScope,
) -> Result<Vec<SchemaIR>, CodegenError> {
    let mut out = Vec::new();
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
        None => return Ok(out),
    };

    let scope_borrow = main_pkg_scopes.borrow();
    for (name, obj_rc) in scope_borrow.elems.iter() {
        let obj = obj_rc.borrow();
        let ty = match &obj.ty.kind {
            TypeKind::Schema(schema_ty) => schema_ty.clone(),
            _ => continue,
        };
        // Mixins are transparent per Phase 4A D10 memo: the consuming
        // schema gets all the mixed-in fields inline; emit no
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
            location: format!("schema {} ({})", name, ty.filename),
        });
    }

    let mut fields = Vec::with_capacity(ty.attrs.len());
    for (field_name, attr) in ty.attrs.iter() {
        let kind = field_kind_for(&attr.ty.kind);
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

    Ok(SchemaIR {
        name,
        source_file: ty.filename.clone(),
        source_line: 0,
        fields,
        doc: ty.doc.clone(),
    })
}

fn field_kind_for(ty_kind: &TypeKind) -> FieldKind {
    match ty_kind {
        TypeKind::Int | TypeKind::IntLit(_) => FieldKind::Int,
        TypeKind::Float | TypeKind::FloatLit(_) => FieldKind::Float,
        TypeKind::Bool | TypeKind::BoolLit(_) => FieldKind::Bool,
        TypeKind::Str | TypeKind::StrLit(_) => FieldKind::Str,
        TypeKind::List(item_ty) => FieldKind::List(Box::new(field_kind_for(&item_ty.kind))),
        TypeKind::Dict(dict_ty) => FieldKind::Dict(
            Box::new(field_kind_for(&dict_ty.key_ty.kind)),
            Box::new(field_kind_for(&dict_ty.val_ty.kind)),
        ),
        TypeKind::Schema(schema_ty) => FieldKind::Schema(schema_ty.name.clone()),
        TypeKind::Function(_) => FieldKind::Unsupported(
            "function-typed field (D4 corollary: codegen-time error)".to_string(),
        ),
        TypeKind::Union(_) => FieldKind::Unsupported(
            "union type (Phase 4 step 5 will handle this)".to_string(),
        ),
        TypeKind::NumberMultiplier(_) => {
            FieldKind::Unsupported("unit-typed value (deferred from Phase 4 MVP)".to_string())
        }
        TypeKind::Any => FieldKind::Unsupported("any-typed field".to_string()),
        TypeKind::None => FieldKind::Unsupported("None-typed field".to_string()),
        TypeKind::Void => FieldKind::Unsupported("Void-typed field".to_string()),
        TypeKind::Module(_) => FieldKind::Unsupported("module-typed field".to_string()),
        TypeKind::Named(name) => FieldKind::Unsupported(format!("Named type alias `{name}`")),
    }
}
