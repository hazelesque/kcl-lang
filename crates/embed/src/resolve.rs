//! Mokkan deferred-resolution types (F2.7).
//!
//! `Resolvable<T>` is the codegen-target type for schema fields
//! declared as `T | ResolvableString`. Generated `TryFrom<&ValueRef>`
//! impls (F2.7b) produce either:
//!
//!   * `Resolvable::Resolved(T)` — the eager case, with the typed
//!     value already parsed from the operator-written literal.
//!   * `Resolvable::Pending(ResolvableString)` — the deferred case,
//!     carrying the symbolic-IR-bearing segment list for the resolver
//!     to substitute at resolve time.
//!
//! Plus mirror re-exports of the runtime-side `Segment` / `Expr` /
//! `ResolvableString` / `IpFamily` types so consumers can write
//! `kcl_embed::resolve::Expr` rather than depending on
//! `kcl_runtime` directly. The canonical definitions live in
//! `kcl_runtime::value::inet` and this module re-exports them as a
//! single typed surface for the public API.
//!
//! Synthesis-side helpers (`pending_subnet`, `pending_inet`,
//! `pending_from_segments`) let Rust consumers build `Pending`
//! values without going through KCL evaluation. T3.1's Rust-side
//! synthesis path uses these.

use kcl_runtime::ValueRef;

// ---------------------------------------------------------------
// Mirror re-exports of the runtime types.
// ---------------------------------------------------------------

pub use kcl_runtime::value::{Expr, IpFamily, ResolvableString, Segment};

/// Eager-evaluation result produced by the resolver. F2.7a defines
/// the shape; T4 (Tilley's resolver) walks symbolic Expr trees
/// against the IPAM allocation map and produces these.
///
/// Carried by the consumer-side TryFrom logic when dispatching on
/// the symbolic-segment Expr's evaluation result — see the plan's
/// D5 typed-Resolvable dispatch: for `Resolvable<T>` where T is not
/// `String`, the resolver evaluates the single-Symbolic-segment
/// Expr to one of these variants and asserts it matches T.
#[derive(Clone, Debug, PartialEq)]
pub enum EagerValue {
    /// Parsed cidr (the result of evaluating `HandleSubnet` /
    /// `NetworkOf(...)` against the resolver's IPAM map).
    Cidr(cidr::IpCidr),
    /// Parsed inet (the result of evaluating `AddOffset(...)` /
    /// `BroadcastOf(...)` / etc.).
    Inet(cidr::IpInet),
    /// Plain int (the result of evaluating `MaskLen(...)` or a
    /// LiteralInt).
    Int(i64),
    /// Stringified value (the result of evaluating
    /// `Text` / `Host` / `Abbrev`).
    String(String),
}

/// Schema-field carrier for `T | ResolvableString` declarations.
///
/// Codegen (F2.7b) emits `Resolvable<T>` for any schema field
/// declared as `T | ResolvableString`. Two variants:
///
///   * `Resolved(T)` — eager case. The operator wrote a literal
///     value (or one that the F1.3 string-coercion handled at
///     schema-validation time).
///   * `Pending(ResolvableString)` — deferred case. The operator
///     wrote a symbolic value (`net_symbolic.symbolic_inet("lan",
///     10)` etc.); the resolver substitutes the typed value at
///     resolve time per the D5 schema discipline.
///
/// The Pending arm carries a `ResolvableString`, not the bare
/// `Expr` IR, because that's the F2.3 carrier and what runtime
/// stringification produces. For non-String T, F2.6 schema
/// discipline guarantees the segment list is exactly one
/// `Segment::Symbolic(expr)`; the resolver's typed dispatch walks
/// that single segment's Expr directly.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolvable<T> {
    /// Eager case: the operator wrote a literal value or one that
    /// the F1.3 string-coercion handled at schema-validation time.
    /// Consumer code reads the inner `T` directly without going
    /// through the resolver pass.
    Resolved(T),
    /// Deferred case: the operator wrote a symbolic value (e.g.,
    /// `net_symbolic.symbolic_inet("lan", 10)`). The
    /// `ResolvableString` carries the segment list (a single
    /// `Segment::Symbolic(Expr)` for non-String T per F2.6 D5
    /// discipline). The resolver substitutes the typed value at
    /// resolve time.
    Pending(ResolvableString),
}

impl<T> Resolvable<T> {
    /// Whether the value is already-resolved (eager case). Useful
    /// for consumer code that wants to short-circuit
    /// resolution-pass logic when no symbolic values are present.
    #[inline]
    pub fn is_resolved(&self) -> bool {
        matches!(self, Resolvable::Resolved(_))
    }

    /// Whether the value is pending resolution (deferred case).
    #[inline]
    pub fn is_pending(&self) -> bool {
        matches!(self, Resolvable::Pending(_))
    }

    /// Extract the resolved value or panic with `field_path`
    /// context. Used by consumer call sites that have already run
    /// the resolver pass — anything still Pending at this point
    /// is a consumer-side bug.
    #[inline]
    pub fn expect_resolved(&self, field_path: &str) -> &T {
        match self {
            Resolvable::Resolved(t) => t,
            Resolvable::Pending(_) => panic!(
                "Resolvable::Pending at {field_path} — the resolver pass should have \
                 substituted this value before consumption (mokkan T4 / Tilley resolver)"
            ),
        }
    }
}

// ---------------------------------------------------------------
// Synthesis-side constructors.
// ---------------------------------------------------------------
//
// Rust consumers (Tilley T3.1's network-definition synthesis,
// future Withy / monorepo-mokkan-pipeline consumers) construct
// `Pending` values directly from handle-shaped data they already
// have in hand. The shapes here parallel the mokkan-source
// builtins from F2.5 (`net_symbolic.symbolic_subnet` /
// `symbolic_inet`); the result types are `Resolvable<T>` rather
// than runtime ValueRef because consumers traffic in their
// codegen-emitted Rust types, not KCL values.

/// Synthesis-side: build a `Resolvable<T>::Pending` carrying a
/// `HandleSubnet { handle, size: Some(size), family: Some(family) }`
/// leaf. Mirrors `net_symbolic.symbolic_subnet` at the Rust-API
/// level. Caller chooses T (typically `cidr::IpCidr` for a
/// `cidr | ResolvableString` field).
pub fn pending_subnet<T>(handle: impl Into<String>, size: u8, family: IpFamily) -> Resolvable<T> {
    let expr = Expr::HandleSubnet {
        handle: handle.into(),
        size: Some(size),
        family: Some(family),
    };
    let rs = *ResolvableString::from_symbolic(expr);
    Resolvable::Pending(rs)
}

/// Synthesis-side: build a `Resolvable<T>::Pending` carrying
/// `AddOffset(NetworkOf(HandleSubnet { handle, size: None, family:
/// None }), LiteralInt(offset))`. Mirrors
/// `net_symbolic.symbolic_inet` at the Rust-API level.
pub fn pending_inet<T>(handle: impl Into<String>, offset: i64) -> Resolvable<T> {
    let handle_expr = Expr::HandleSubnet {
        handle: handle.into(),
        size: None,
        family: None,
    };
    let expr = Expr::AddOffset(
        Box::new(Expr::NetworkOf(Box::new(handle_expr))),
        Box::new(Expr::LiteralInt(offset)),
    );
    let rs = *ResolvableString::from_symbolic(expr);
    Resolvable::Pending(rs)
}

/// Synthesis-side: build a `Resolvable<T>::Pending` from a
/// pre-built segment list. Escape hatch for consumers that need to
/// construct arbitrary segment sequences (e.g., a synthesis path
/// mixing literal text with multiple symbolic substitutions).
/// Typically `pending_subnet` / `pending_inet` are sufficient.
pub fn pending_from_segments<T>(segments: Vec<Segment>) -> Resolvable<T> {
    Resolvable::Pending(*ResolvableString::from_segments(segments))
}

// ---------------------------------------------------------------
// Diagnostic extraction from a `ValueRef`.
// ---------------------------------------------------------------

/// Extract a `ResolvableString` reference from a ValueRef that
/// holds one. Used by F2.7b's codegen-emitted TryFrom bodies — the
/// emitted code dispatches on `value.is_resolvable_string()` first
/// (Pending arm) before falling through to the eager-T conversion.
///
/// Returns `None` if the ValueRef doesn't hold a
/// `resolvable_string_value` variant. Codegen call sites first
/// check `value.is_resolvable_string()` to disambiguate.
pub fn as_resolvable_string(value: &ValueRef) -> Option<ResolvableString> {
    match &*value.rc.borrow() {
        kcl_runtime::Value::resolvable_string_value(rs) => Some((**rs).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDLE: &str = "lan";

    /// Synthesis path constructs a Pending Resolvable carrying a
    /// HandleSubnet IR with both Option fields Some(asserted). The
    /// resolver later checks Some(asserted) against the network's
    /// declared values.
    #[test]
    fn pending_subnet_carries_handle_subnet_with_some_size_and_family() {
        let r: Resolvable<cidr::IpCidr> = pending_subnet(HANDLE, 24, IpFamily::V4);
        assert!(r.is_pending());
        match r {
            Resolvable::Pending(rs) => {
                assert_eq!(rs.segments.len(), 1, "single Symbolic segment expected");
                match &rs.segments[0] {
                    Segment::Symbolic(e) => match e.as_ref() {
                        Expr::HandleSubnet {
                            handle,
                            size,
                            family,
                        } => {
                            assert_eq!(handle, HANDLE);
                            assert_eq!(*size, Some(24));
                            assert_eq!(*family, Some(IpFamily::V4));
                        }
                        other => panic!("expected HandleSubnet, got {other:?}"),
                    },
                    other => panic!("expected Symbolic segment, got {other:?}"),
                }
            }
            Resolvable::Resolved(_) => panic!("expected Pending"),
        }
    }

    /// Synthesis path for symbolic_inet builds
    /// `AddOffset(NetworkOf(HandleSubnet{None,None}), LiteralInt(N))`
    /// — both Option fields None on the leaf because the call site
    /// asserted neither.
    #[test]
    fn pending_inet_carries_addoffset_of_networkof_handle_subnet() {
        let r: Resolvable<cidr::IpInet> = pending_inet(HANDLE, 10);
        assert!(r.is_pending());
        match r {
            Resolvable::Pending(rs) => match &rs.segments[0] {
                Segment::Symbolic(e) => match e.as_ref() {
                    Expr::AddOffset(net_box, lit_box) => {
                        match net_box.as_ref() {
                            Expr::NetworkOf(handle_box) => match handle_box.as_ref() {
                                Expr::HandleSubnet {
                                    handle,
                                    size,
                                    family,
                                } => {
                                    assert_eq!(handle, HANDLE);
                                    assert_eq!(*size, None);
                                    assert_eq!(*family, None);
                                }
                                other => panic!("expected HandleSubnet, got {other:?}"),
                            },
                            other => panic!("expected NetworkOf, got {other:?}"),
                        }
                        match lit_box.as_ref() {
                            Expr::LiteralInt(n) => assert_eq!(*n, 10),
                            other => panic!("expected LiteralInt, got {other:?}"),
                        }
                    }
                    other => panic!("expected AddOffset, got {other:?}"),
                },
                other => panic!("expected Symbolic segment, got {other:?}"),
            },
            Resolvable::Resolved(_) => panic!("expected Pending"),
        }
    }

    /// Generic-segment-list constructor preserves the operator-
    /// supplied segments verbatim — no normalisation, no
    /// re-shaping. Locks down the escape-hatch contract.
    #[test]
    fn pending_from_segments_preserves_segment_list() {
        let segs = vec![
            Segment::Literal("prefix-".to_string()),
            Segment::Symbolic(Box::new(Expr::HandleSubnet {
                handle: HANDLE.to_string(),
                size: None,
                family: None,
            })),
            Segment::Literal("-suffix".to_string()),
        ];
        let r: Resolvable<String> = pending_from_segments(segs.clone());
        match r {
            Resolvable::Pending(rs) => assert_eq!(rs.segments, segs),
            Resolvable::Resolved(_) => panic!("expected Pending"),
        }
    }

    /// `expect_resolved` panic message names the field_path so the
    /// consumer-side diagnostic points at the offending site.
    #[test]
    #[should_panic(expected = "Resolvable::Pending at vms.alpha.command")]
    fn expect_resolved_panic_includes_field_path() {
        let r: Resolvable<String> = pending_from_segments(vec![Segment::Literal("x".to_string())]);
        let _ = r.expect_resolved("vms.alpha.command");
    }

    /// `expect_resolved` on a Resolved value returns the wrapped T
    /// — the eager case is the happy path.
    #[test]
    fn expect_resolved_returns_wrapped_value() {
        let r: Resolvable<String> = Resolvable::Resolved("hello".to_string());
        assert_eq!(r.expect_resolved("vms.alpha.x"), "hello");
    }

    /// `as_resolvable_string` extracts a clone from a ValueRef
    /// holding a `resolvable_string_value`. Round-trips through the
    /// runtime value wrapper.
    #[test]
    fn as_resolvable_string_extracts_from_value_ref() {
        let rs_inner = ResolvableString::from_literal("hello");
        let v = ValueRef::from(kcl_runtime::Value::resolvable_string_value(rs_inner));
        let extracted = as_resolvable_string(&v).expect("should extract");
        assert_eq!(extracted.segments.len(), 1);
        match &extracted.segments[0] {
            Segment::Literal(s) => assert_eq!(s, "hello"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    /// Non-RS ValueRef returns None.
    #[test]
    fn as_resolvable_string_returns_none_for_other_variants() {
        let v = ValueRef::str("hello");
        assert!(as_resolvable_string(&v).is_none());
    }
}
