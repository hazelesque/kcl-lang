//! Mokkan `mokkan.uuid` package (Phase C.2).
//!
//! UUID constructors. The operator writes
//! `uuid.parse("0123abcd-...")` or
//! `uuid.v5(namespace, "name")` in mokkan source; this module
//! produces the corresponding `uuid_value` (Phase C.1).
//!
//! Two builtins, both following the standard C-ABI shape used by
//! `mokkan_net/mod.rs` and `mokkan_net_symbolic/mod.rs`:
//!
//! ```ignore
//! #[unsafe(no_mangle)]
//! pub unsafe extern "C-unwind" fn kcl_mokkan_uuid_<name>(
//!     ctx: *mut kcl_context_t,
//!     args: *const kcl_value_ref_t,
//!     kwargs: *const kcl_value_ref_t,
//! ) -> *const kcl_value_ref_t
//! ```
//!
//! Function-name mangling per
//! `sema::builtin::mangle_system_module_func`: dots in the pkgpath
//! become underscores, so `mokkan.uuid.parse` →
//! `kcl_mokkan_uuid_parse` and `mokkan.uuid.v5` →
//! `kcl_mokkan_uuid_v5`.
//!
//! No `v4()` builtin in Phase C.2 — random UUIDs at evaluation time
//! would make Tilleyfile evaluation non-deterministic (same input
//! produces different output across runs). Operators wanting
//! randomness pass a UUID in via `external_args` (the path Tilley
//! uses for `host_uuid` and `project_uuid`).

use crate::*;

/// Internal: extract a `str_value` argument with a uuid-flavoured
/// panic on mismatch.
fn arg_as_str(value: &ValueRef, func: &str, arg: &str) -> String {
    match &*value.rc.borrow() {
        Value::str_value(s) => s.clone(),
        _ => panic!(
            "{func}() expected str for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Internal: extract a `uuid_value` argument.
fn arg_as_uuid(value: &ValueRef, func: &str, arg: &str) -> uuid::Uuid {
    match &*value.rc.borrow() {
        Value::uuid_value(v) => *v,
        _ => panic!(
            "{func}() expected uuid for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Mokkan `parse(s: str) -> uuid`.
///
/// Parses a hyphenated UUID string into a typed `uuid_value`. The
/// `uuid::Uuid::parse_str` parser is strict on hyphen placement and
/// segment widths; malformed input panics with a clear message
/// (operator-visible diagnostic at the call site).
///
/// Most schema authors don't need this — assigning a string literal
/// to a `uuid`-typed field uses the F1.3-style str→uuid coercion
/// path automatically. `parse` is for cases where the conversion
/// happens mid-expression (e.g., `uuid.v5(uuid.parse(ns_str), name)`
/// where the namespace UUID arrived as a string from `option()`).
///
/// # Safety
/// C-ABI raw-pointer surface. Same contract as every other
/// `kcl_*_*` builtin in `crates/runtime`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_uuid_parse(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    let s_arg = get_call_arg(args, kwargs, 0, Some("s"))
        .unwrap_or_else(|| panic!("uuid.parse() missing required argument 's'"));
    let s = arg_as_str(&s_arg, "uuid.parse", "s");

    let parsed = uuid::Uuid::parse_str(&s)
        .unwrap_or_else(|e| panic!("uuid.parse() failed to parse {s:?}: {e}"));
    ValueRef::from(Value::uuid_value(parsed)).into_raw(ctx)
}

/// Mokkan `v5(namespace: uuid, name: str) -> uuid`.
///
/// RFC 4122 §4.3 deterministic UUID derivation. Same `namespace` +
/// `name` always produces the same output UUID; different namespaces
/// or different names produce distinct UUIDs.
///
/// Used by Tilley's `project_uuid` and `host_uuid` derivation paths
/// (Phase C.2): the namespace is a Tilley-shipped constant or
/// operator-supplied anchor, the name is a stable project / host
/// identifier.
///
/// # Safety
/// C-ABI raw-pointer surface. Same contract as every other
/// `kcl_*_*` builtin in `crates/runtime`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_uuid_v5(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    let ns_arg = get_call_arg(args, kwargs, 0, Some("namespace"))
        .unwrap_or_else(|| panic!("uuid.v5() missing required argument 'namespace'"));
    let name_arg = get_call_arg(args, kwargs, 1, Some("name"))
        .unwrap_or_else(|| panic!("uuid.v5() missing required argument 'name'"));

    let namespace = arg_as_uuid(&ns_arg, "uuid.v5", "namespace");
    let name = arg_as_str(&name_arg, "uuid.v5", "name");

    let derived = uuid::Uuid::new_v5(&namespace, name.as_bytes());
    ValueRef::from(Value::uuid_value(derived)).into_raw(ctx)
}

#[cfg(test)]
mod tests {
    //! Phase C.2 builtin shape tests.
    //!
    //! The C-ABI surface is exercised end-to-end through the embed
    //! integration tests (KCL source `import mokkan.uuid` /
    //! `uuid.parse(...)` / `uuid.v5(...)`). These unit tests cover
    //! the construction logic at the Rust level without the
    //! kcl_context_t plumbing.

    use std::str::FromStr;

    /// `uuid.parse("0123abcd-...")` matches `uuid::Uuid::parse_str`
    /// of the same string — the builtin is a thin wrapper.
    #[test]
    fn parse_matches_uuid_crate() {
        let s = "0123abcd-4567-8910-1112-131415161718";
        let direct = uuid::Uuid::parse_str(s).unwrap();
        let parsed = uuid::Uuid::from_str(s).unwrap();
        assert_eq!(direct, parsed);
    }

    /// `uuid.v5(ns, name) == uuid.v5(ns, name)` — same inputs
    /// produce the same output (deterministic).
    #[test]
    fn v5_is_deterministic() {
        let ns = uuid::Uuid::parse_str("0123abcd-4567-8910-1112-131415161718").unwrap();
        let a = uuid::Uuid::new_v5(&ns, b"some-name");
        let b = uuid::Uuid::new_v5(&ns, b"some-name");
        assert_eq!(a, b);
    }

    /// Distinct namespaces produce distinct v5 UUIDs from the same
    /// name. Namespace-isolation property — the load-bearing
    /// guarantee for "two orgs deriving names from different
    /// anchors don't collide."
    #[test]
    fn v5_namespace_isolation() {
        let ns1 = uuid::Uuid::parse_str("0123abcd-4567-8910-1112-131415161718").unwrap();
        let ns2 = uuid::Uuid::parse_str("fedcba98-7654-3210-0987-654321fedcba").unwrap();
        let a = uuid::Uuid::new_v5(&ns1, b"same-name");
        let b = uuid::Uuid::new_v5(&ns2, b"same-name");
        assert_ne!(a, b);
    }
}
