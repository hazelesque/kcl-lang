//! Mokkan `mokkan.net_symbolic` package (F2.5).
//!
//! Symbolic-primitive constructors. The operator writes
//! `net_symbolic.symbolic_subnet("lan", 24, net.V4)` in mokkan
//! source; this module produces the corresponding symbolic
//! `cidr_value` carrying a `HandleSubnet` leaf in the F2.1 `Expr`
//! IR. Downstream algebra (broadcast, network, +, -, …) composes
//! the IR via F2.2's dispatch.
//!
//! Two builtins, both following the standard C-ABI shape used by
//! `mokkan_net/mod.rs`:
//!
//! ```ignore
//! #[unsafe(no_mangle)]
//! pub unsafe extern "C-unwind" fn kcl_mokkan_net_symbolic_<name>(
//!     ctx: *mut kcl_context_t,
//!     args: *const kcl_value_ref_t,
//!     kwargs: *const kcl_value_ref_t,
//! ) -> *const kcl_value_ref_t
//! ```
//!
//! Function-name mangling per
//! `sema::builtin::mangle_system_module_func`: dots in the pkgpath
//! become underscores, so
//! `mokkan.net_symbolic.symbolic_subnet` →
//! `kcl_mokkan_net_symbolic_symbolic_subnet`. Yes, the double
//! `symbolic_` reads awkwardly — the package is `net_symbolic` and
//! the function is `symbolic_subnet`. Renaming the function to
//! just `subnet` would be cleaner at the symbol-name level but
//! lossier at the call site (`net_symbolic.subnet("lan", …)` reads
//! like a constructor, which it is, but the matching
//! `symbolic_subnet` keeps the symbolic semantics in the function
//! name where the operator sees it).
//!
//! `mokkan.symbolic` (without `.net_`) is reserved per D6 for
//! generic deferred-value primitives (e.g., direct
//! `ResolvableString` constructors from KCL). Empty in F2; trivial
//! to add when a use case surfaces.

use crate::*;

/// Internal: extract a `str_value` argument with a F2.5-flavoured
/// panic on mismatch.
fn arg_as_handle_str(value: &ValueRef, func: &str) -> String {
    match &*value.rc.borrow() {
        Value::str_value(s) => s.clone(),
        _ => panic!(
            "{func}() expected str for argument 'handle', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Internal: extract an `int_value` argument.
fn arg_as_int(value: &ValueRef, func: &str, arg: &str) -> i64 {
    match &*value.rc.borrow() {
        Value::int_value(v) => *v,
        _ => panic!(
            "{func}() expected int for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Internal: extract an `ip_family_value` argument.
fn arg_as_ip_family(value: &ValueRef, func: &str, arg: &str) -> crate::value::IpFamily {
    match &*value.rc.borrow() {
        Value::ip_family_value(v) => *v,
        _ => panic!(
            "{func}() expected IpFamily for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Mokkan `symbolic_subnet(handle: str, size: int, family: IpFamily) -> cidr`.
///
/// Produces a symbolic `cidr_value` carrying a single
/// `HandleSubnet { handle, size: Some(size), family: Some(family) }`
/// leaf — both `Some(...)` per D3 because the operator asserted
/// both values at the call site; the resolver validates both
/// against the network's declaration at resolve time.
///
/// Family is required (per D6 / Hazel's "no v4 baked in forever"
/// rule). Sema-level signature enforces it.
///
/// # Safety
/// C-ABI raw-pointer surface. Same contract as every other
/// `kcl_*_*` builtin in `crates/runtime`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_symbolic_symbolic_subnet(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    let handle_arg = get_call_arg(args, kwargs, 0, Some("handle"))
        .unwrap_or_else(|| panic!("symbolic_subnet() missing required argument 'handle'"));
    let size_arg = get_call_arg(args, kwargs, 1, Some("size"))
        .unwrap_or_else(|| panic!("symbolic_subnet() missing required argument 'size'"));
    let family_arg = get_call_arg(args, kwargs, 2, Some("family"))
        .unwrap_or_else(|| panic!("symbolic_subnet() missing required argument 'family'"));

    let handle = arg_as_handle_str(&handle_arg, "symbolic_subnet");
    let size = arg_as_int(&size_arg, "symbolic_subnet", "size");
    let family = arg_as_ip_family(&family_arg, "symbolic_subnet", "family");

    // Range-check the size at the operator-facing point. v4 0..=32,
    // v6 0..=128. The check is family-aware here because we have it
    // eagerly; the matching range-check inside SetMasklen (F2.2)
    // can't do this because Expr::HandleSubnet on its own doesn't
    // carry the family at consumption time.
    let max_mask = match family {
        crate::value::IpFamily::V4 => 32,
        crate::value::IpFamily::V6 => 128,
    };
    if size < 0 || size > max_mask {
        panic!(
            "symbolic_subnet() size {size} out of range for {family} \
             (expected 0..={max_mask})"
        );
    }

    let expr = crate::value::Expr::HandleSubnet {
        handle,
        size: Some(size as u8),
        family: Some(family),
    };
    ValueRef::from(Value::cidr_value(crate::value::CidrValue::symbolic(expr))).into_raw(ctx)
}

/// Mokkan `symbolic_inet(handle: str, offset: int) -> inet`.
///
/// Produces a symbolic `inet_value` carrying
/// `AddOffset(NetworkOf(HandleSubnet { handle, size: None, family: None }), LiteralInt(offset))`
/// — both `size: None` and `family: None` per D3 because the
/// attachment doesn't declare either at the call site; the
/// resolver looks them up from the handle's network declaration
/// without validation. This parallels `symbolic_subnet`'s
/// `Some(size)` / `Some(family)`, where the operator did assert
/// values and the resolver validates them.
///
/// The composition `AddOffset(NetworkOf(handle), offset)` expresses
/// "address at offset N within the network's first-usable range" —
/// the resolver evaluates `NetworkOf(handle)` against the IPAM
/// allocation map to get the resolved cidr, then `AddOffset(...,
/// N)` produces the N-th address from the network's base.
///
/// # Safety
/// C-ABI raw-pointer surface. Same contract as every other
/// `kcl_*_*` builtin in `crates/runtime`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_symbolic_symbolic_inet(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    let handle_arg = get_call_arg(args, kwargs, 0, Some("handle"))
        .unwrap_or_else(|| panic!("symbolic_inet() missing required argument 'handle'"));
    let offset_arg = get_call_arg(args, kwargs, 1, Some("offset"))
        .unwrap_or_else(|| panic!("symbolic_inet() missing required argument 'offset'"));

    let handle = arg_as_handle_str(&handle_arg, "symbolic_inet");
    let offset = arg_as_int(&offset_arg, "symbolic_inet", "offset");

    // Offset can be negative — operator wants "the address N before
    // the network's end" or similar. Range-check happens at resolve
    // time when the family + size are known.

    let handle_expr = crate::value::Expr::HandleSubnet {
        handle,
        size: None,
        family: None,
    };
    let expr = crate::value::Expr::AddOffset(
        Box::new(crate::value::Expr::NetworkOf(Box::new(handle_expr))),
        Box::new(crate::value::Expr::LiteralInt(offset)),
    );
    ValueRef::from(Value::inet_value(crate::value::InetValue::symbolic(expr))).into_raw(ctx)
}

#[cfg(test)]
mod tests {
    //! F2.5 builtin shape tests.
    //!
    //! The C-ABI surface is exercised end-to-end through the embed
    //! integration tests (KCL source `import mokkan.net_symbolic` /
    //! `net_symbolic.symbolic_subnet(...)`). These unit tests cover
    //! the construction logic at the Rust-level — the Expr shape the
    //! C-ABI functions would produce — without needing the
    //! kcl_context_t plumbing.

    use crate::value::{CidrInner, Expr, InetInner, IpFamily};

    const HANDLE: &str = "lan";

    /// `symbolic_subnet("lan", 24, V4)` produces a HandleSubnet with
    /// both Option fields Some(asserted-value). This is the load-
    /// bearing D3 shape — the resolver later checks Some(asserted)
    /// against the network's declaration.
    #[test]
    fn symbolic_subnet_construction_shape() {
        // Mirror what kcl_mokkan_net_symbolic_symbolic_subnet builds
        // on its symbolic path, without going through C-ABI.
        let expr = Expr::HandleSubnet {
            handle: HANDLE.to_string(),
            size: Some(24),
            family: Some(IpFamily::V4),
        };
        let cv = crate::value::CidrValue::symbolic(expr.clone());
        assert!(cv.is_symbolic());
        match &cv.inner {
            CidrInner::Symbolic(got) => assert_eq!(*got, expr),
            CidrInner::Resolved(_) => panic!("expected Symbolic inner"),
        }
    }

    /// `symbolic_inet("lan", 10)` produces
    /// `AddOffset(NetworkOf(HandleSubnet{None,None}), LiteralInt(10))`.
    /// Both Option fields on the leaf are None because the call site
    /// doesn't assert size or family; the resolver inherits both
    /// from the handle's network declaration.
    #[test]
    fn symbolic_inet_construction_shape() {
        let handle_expr = Expr::HandleSubnet {
            handle: HANDLE.to_string(),
            size: None,
            family: None,
        };
        let expr = Expr::AddOffset(
            Box::new(Expr::NetworkOf(Box::new(handle_expr))),
            Box::new(Expr::LiteralInt(10)),
        );
        let iv = crate::value::InetValue::symbolic(expr.clone());
        assert!(iv.is_symbolic());
        match &iv.inner {
            InetInner::Symbolic(got) => assert_eq!(*got, expr),
            InetInner::Resolved(_) => panic!("expected Symbolic inner"),
        }
    }
}
