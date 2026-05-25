//! Parse `# @rust: ...` annotations from KCL source comments.
//!
//! Phase 4B introduces opt-in codegen behaviour driven by source-level
//! annotations. The current set:
//!
//! - **`# @rust: tagged_enum(discriminator = "<field>")`** — applied
//!   to a schema declaration line. Tells codegen to emit a Rust
//!   `enum` with one variant per literal value of the named
//!   discriminator field, rather than the default flat-struct shape.
//!   Each variant carries the fields that the user has assigned to
//!   it via per-field `variant(...)` / `shared` annotations.
//!
//! - **`# @rust: variant("<name>")`** — applied to a field. Declares
//!   that the field belongs to exactly one variant of an enclosing
//!   tagged-enum schema. The field's `?:`-declared optionality is
//!   collapsed: within its assigned variant, the field is *required*
//!   (the schema's `check:` block enforces this), so codegen emits
//!   it as a non-`Option<T>` field of that variant.
//!
//! - **`# @rust: shared`** — applied to a field. Declares the field
//!   appears in every variant. The field's `?:`-declared optionality
//!   is preserved (shared `?:` fields become `Option<T>` in every
//!   variant; shared required fields stay `T`).
//!
//! Annotations are parsed from the AST's preserved `Comment` nodes
//! via line-matching: a schema-level annotation lives on the line
//! immediately above `schema X:`; a field-level annotation lives as
//! a trailing comment on the same line as the field declaration.

use std::collections::HashMap;

use crate::CodegenError;

/// All `@rust:` annotations attached to a single schema.
#[derive(Debug, Clone, Default)]
pub(crate) struct SchemaAnnotations {
    /// Schema-level `tagged_enum(discriminator = "<field>")` if
    /// present. `None` means the schema codegens as a struct (the
    /// pre-Phase-4B default).
    pub tagged_enum: Option<TaggedEnumAnnotation>,
    /// Per-field annotations keyed by field name. Empty when the
    /// schema is not tagged-enum-annotated; populated only when the
    /// schema has been promoted to tagged-enum codegen.
    pub fields: HashMap<String, FieldAnnotation>,
}

/// Schema-level `# @rust: tagged_enum(discriminator = "X")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaggedEnumAnnotation {
    /// Name of the discriminator field on the same schema. Validated
    /// at IR-extraction time to exist and to be a string-literal
    /// union (so its variants enumerate the enum's variants).
    pub discriminator: String,
}

/// Per-field annotation. A field has at most one annotation; mixing
/// `variant(...)` and `shared` on the same field is rejected at
/// parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldAnnotation {
    /// `# @rust: variant("X")` or `# @rust: variant("X", optional)`.
    /// Field belongs to variant X only. The `is_variant_optional`
    /// flag controls how the field's KCL-level optionality maps to
    /// Rust:
    ///
    /// - `false` (default — bare `variant("X")`): the field is
    ///   *required within variant X* (the schema's check: block
    ///   enforces it), so codegen collapses `field?: T` to `T` in
    ///   the variant.
    /// - `true` (`variant("X", optional)`): the field remains
    ///   *optional within variant X*, so codegen emits `Option<T>`.
    ///   Use this when the KCL check block doesn't require the
    ///   field for the variant.
    Variant {
        name: String,
        is_variant_optional: bool,
    },
    /// `# @rust: shared` — field appears in every variant.
    Shared,
}

/// Parse one comment line for an `@rust:` annotation.
///
/// Returns:
/// - `Ok(Some(_))` if the comment is a recognised `@rust:`
///   annotation.
/// - `Ok(None)` if the comment is not an `@rust:` annotation at all
///   (just a regular KCL comment).
/// - `Err(_)` if the comment starts with `@rust:` but is malformed
///   (typo, unknown directive, etc.). Surfaced as
///   `CodegenError::InvalidAnnotation` to fail loudly rather than
///   silently mis-parsing.
pub(crate) fn parse_rust_annotation(
    comment_text: &str,
) -> Result<Option<RawAnnotation>, CodegenError> {
    // Strip leading `#` if present (KCL comments include it).
    let stripped = comment_text.trim_start_matches('#').trim_start();
    if !stripped.starts_with("@rust:") {
        return Ok(None);
    }
    let body = stripped["@rust:".len()..].trim();

    // Schema-level: `tagged_enum(discriminator = "X")`
    if let Some(rest) = body.strip_prefix("tagged_enum(") {
        let inner = rest
            .strip_suffix(')')
            .ok_or_else(|| {
                CodegenError::InvalidAnnotation(format!(
                    "tagged_enum: missing closing `)` in {comment_text:?}"
                ))
            })?
            .trim();
        let discriminator = parse_kv_string(inner, "discriminator").ok_or_else(|| {
            CodegenError::InvalidAnnotation(format!(
                "tagged_enum: expected `discriminator = \"<field>\"`, got {comment_text:?}"
            ))
        })?;
        return Ok(Some(RawAnnotation::TaggedEnum(TaggedEnumAnnotation {
            discriminator,
        })));
    }

    // Field-level: `variant("X")` or `variant("X", optional)`
    if let Some(rest) = body.strip_prefix("variant(") {
        let inner = rest
            .strip_suffix(')')
            .ok_or_else(|| {
                CodegenError::InvalidAnnotation(format!(
                    "variant: missing closing `)` in {comment_text:?}"
                ))
            })?
            .trim();
        // Split on the first comma (if any). The grammar is
        // intentionally narrow: the name is a quoted string; the
        // optional second arg is the literal identifier `optional`.
        let (name_part, modifier_part) = match inner.split_once(',') {
            Some((a, b)) => (a.trim(), Some(b.trim())),
            None => (inner, None),
        };
        let name = parse_string_literal(name_part).ok_or_else(|| {
            CodegenError::InvalidAnnotation(format!(
                "variant: expected `variant(\"<name>\")` or `variant(\"<name>\", optional)`, got {comment_text:?}"
            ))
        })?;
        let is_variant_optional = match modifier_part {
            None => false,
            Some("optional") => true,
            Some(other) => {
                return Err(CodegenError::InvalidAnnotation(format!(
                    "variant: unknown modifier {other:?} (expected `optional` or nothing) in {comment_text:?}"
                )));
            }
        };
        return Ok(Some(RawAnnotation::Field(FieldAnnotation::Variant {
            name,
            is_variant_optional,
        })));
    }

    // Field-level: `shared`
    if body == "shared" {
        return Ok(Some(RawAnnotation::Field(FieldAnnotation::Shared)));
    }

    Err(CodegenError::InvalidAnnotation(format!(
        "unknown @rust: directive in {comment_text:?}; expected one of \
         `tagged_enum(discriminator = \"...\")`, `variant(\"...\")`, `shared`"
    )))
}

/// A parsed annotation, before being attached to a schema or field.
/// The caller decides which slot it goes into based on where the
/// comment lives in the source.
#[derive(Debug, Clone)]
pub(crate) enum RawAnnotation {
    TaggedEnum(TaggedEnumAnnotation),
    Field(FieldAnnotation),
}

/// Match `key = "value"` inside an annotation body. Returns the
/// quoted string content if `key` matches and the value is a valid
/// double-quoted literal; otherwise `None`.
fn parse_kv_string(body: &str, key: &str) -> Option<String> {
    let body = body.trim();
    let prefix = format!("{key}=");
    let prefix_spaces = format!("{key} =");
    let after = if let Some(rest) = body.strip_prefix(&prefix) {
        rest
    } else if let Some(rest) = body.strip_prefix(&prefix_spaces) {
        rest
    } else {
        return None;
    };
    parse_string_literal(after.trim())
}

/// Match a double-quoted string literal `"X"` and return its
/// contents. No escape processing — the annotation grammar doesn't
/// admit escapes (variant names and discriminator field names are
/// identifier-like).
fn parse_string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    if inner.contains('"') {
        return None;
    }
    Some(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_schema_level_tagged_enum() {
        let anno = parse_rust_annotation("# @rust: tagged_enum(discriminator = \"type\")")
            .expect("parse")
            .expect("recognised");
        match anno {
            RawAnnotation::TaggedEnum(t) => assert_eq!(t.discriminator, "type"),
            other => panic!("expected TaggedEnum, got {other:?}"),
        }
    }

    #[test]
    fn parses_field_variant() {
        let anno = parse_rust_annotation("# @rust: variant(\"command\")")
            .expect("parse")
            .expect("recognised");
        match anno {
            RawAnnotation::Field(FieldAnnotation::Variant {
                name,
                is_variant_optional,
            }) => {
                assert_eq!(name, "command");
                assert!(!is_variant_optional);
            }
            other => panic!("expected Variant, got {other:?}"),
        }
    }

    #[test]
    fn parses_field_variant_optional_modifier() {
        let anno = parse_rust_annotation("# @rust: variant(\"file\", optional)")
            .expect("parse")
            .expect("recognised");
        match anno {
            RawAnnotation::Field(FieldAnnotation::Variant {
                name,
                is_variant_optional,
            }) => {
                assert_eq!(name, "file");
                assert!(is_variant_optional, "modifier should set optional flag");
            }
            other => panic!("expected Variant, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_variant_modifier() {
        let err = parse_rust_annotation("# @rust: variant(\"file\", garbage)")
            .expect_err("unknown modifier should error");
        let msg = err.to_string();
        assert!(msg.contains("garbage"), "msg should name the modifier: {msg}");
    }

    #[test]
    fn parses_field_shared() {
        let anno = parse_rust_annotation("# @rust: shared")
            .expect("parse")
            .expect("recognised");
        assert!(matches!(anno, RawAnnotation::Field(FieldAnnotation::Shared)));
    }

    #[test]
    fn returns_none_for_unrelated_comment() {
        let result = parse_rust_annotation("# This is just a regular comment");
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn returns_none_for_empty_comment() {
        let result = parse_rust_annotation("#");
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn rejects_unknown_directive() {
        let err = parse_rust_annotation("# @rust: nonsense(foo)")
            .expect_err("unknown directive should error");
        assert!(matches!(err, CodegenError::InvalidAnnotation(_)));
    }

    #[test]
    fn rejects_tagged_enum_missing_close_paren() {
        let err = parse_rust_annotation("# @rust: tagged_enum(discriminator = \"type\"")
            .expect_err("missing close paren should error");
        let msg = err.to_string();
        assert!(msg.contains("tagged_enum"), "msg should mention tagged_enum: {msg}");
    }

    #[test]
    fn rejects_variant_with_no_quoted_name() {
        let err = parse_rust_annotation("# @rust: variant(command)")
            .expect_err("unquoted name should error");
        let msg = err.to_string();
        assert!(msg.contains("variant"), "msg should mention variant: {msg}");
    }

    #[test]
    fn accepts_extra_whitespace() {
        let anno =
            parse_rust_annotation("#    @rust:   tagged_enum( discriminator = \"type\" )")
                .expect("parse")
                .expect("recognised");
        assert!(matches!(
            anno,
            RawAnnotation::TaggedEnum(TaggedEnumAnnotation { discriminator }) if discriminator == "type"
        ));
    }
}
