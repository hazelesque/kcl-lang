//! Mokkan native `mokkan.net` package (F1.4).
//!
//! Typed inet algebra replacing upstream KCL's stringly-typed `net`
//! package. Functions operate on the F1.1 `cidr_value` / `inet_value`
//! / `macaddr_value` / `macaddr8_value` / `ip_family_value` runtime
//! variants directly — no string parsing, no stringly-typed escape
//! hatch.
//!
//! Function name mangling per `sema::builtin::mangle_system_module_func`:
//! `mokkan.net.broadcast` → `kcl_mokkan_net_broadcast`. The dot in
//! the package path becomes an underscore in the Rust symbol so the
//! linker accepts it.
//!
//! Each builtin follows the standard package-function shape from
//! `regex/mod.rs` / `net/mod.rs`:
//!
//! ```ignore
//! #[unsafe(no_mangle)]
//! pub unsafe extern "C-unwind" fn kcl_mokkan_net_<name>(
//!     ctx: *mut kcl_context_t,
//!     args: *const kcl_value_ref_t,
//!     kwargs: *const kcl_value_ref_t,
//! ) -> *const kcl_value_ref_t
//! ```
//!
//! Required args missing → panic with PG-style "name() missing
//! positional argument" message. Type mismatches → panic with the
//! observed type (the sema layer is supposed to catch these before
//! we get here, so panics are a backstop).
//!
//! F1.4 ships a single-function trial run (`broadcast`); the rest of
//! the PG algebra lands in the bulk-add follow-up. The bulk-add
//! reuses every helper here without restructuring.

use crate::*;

/// Extract an `inet` from a ValueRef. Accepts either a typed
/// `inet_value` directly OR a `str_value` that parses as an inet —
/// the same str → inet coercion F1.3 wires at schema-validation
/// time, applied here so callers can write
/// `net.broadcast("10.0.0.1/24")` without an explicit conversion.
/// Panics if the value is neither shape (sema is expected to
/// catch type mismatches before runtime).
fn arg_as_inet(value: &ValueRef, func: &str, arg: &str) -> cidr::IpInet {
    let coerced = try_coerce_mokkan_inet(value, MOKKAN_TYPE_INET);
    let view = match coerced.as_ref() {
        Some(v) => v.rc.borrow(),
        None => value.rc.borrow(),
    };
    match &*view {
        // F2.1: panic on symbolic operands here — F2.2 introduces
        // per-function symbolic-arm dispatch, which will replace each
        // panic with the typed propagation rule from the D4 matrix.
        // Until then, callers operate on resolved values only.
        Value::inet_value(i) => i.expect_resolved(),
        _ => panic!(
            "{func}() expected inet for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Same as `arg_as_inet` but for the strict `cidr` shape (host
/// bits must be zero — D8 strictness). Used by predicates that
/// only make sense on canonical networks (`contains`, `overlaps`,
/// etc.) and by `inet_merge` for type consistency on its result.
fn arg_as_cidr(value: &ValueRef, func: &str, arg: &str) -> cidr::IpCidr {
    let coerced = try_coerce_mokkan_inet(value, MOKKAN_TYPE_CIDR);
    let view = match coerced.as_ref() {
        Some(v) => v.rc.borrow(),
        None => value.rc.borrow(),
    };
    match &*view {
        Value::cidr_value(c) => c.expect_resolved(),
        _ => panic!(
            "{func}() expected cidr for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
    }
}

/// Same shape as `arg_as_inet` for plain `int`. No coercion; the
/// argument must already be int-shaped at sema time.
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

/// PostgreSQL `host(inet) -> text`: text form of the address
/// without the masklen.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_host(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("host() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "host", "addr");
    ValueRef::str(inet.address().to_string().as_ref()).into_raw(ctx)
}

/// PostgreSQL `masklen(inet) -> int`: the network mask length.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_masklen(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("masklen() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "masklen", "addr");
    ValueRef::int(i64::from(inet.network_length())).into_raw(ctx)
}

/// PostgreSQL `netmask(inet) -> inet`: the netmask as an inet
/// (e.g., 255.255.255.0 for a /24).
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_netmask(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("netmask() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "netmask", "addr");
    let netmask = inet.mask();
    let result =
        cidr::IpInet::new(netmask, inet.network_length()).expect("masklen valid for resolved inet");
    ValueRef::from(Value::inet_value(crate::value::InetValue::resolved(result))).into_raw(ctx)
}

/// PostgreSQL `hostmask(inet) -> inet`: the host mask as an inet
/// (e.g., 0.0.0.255 for a /24 — bitwise complement of netmask).
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_hostmask(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("hostmask() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "hostmask", "addr");
    let netmask = inet.mask();
    let hostmask = match netmask {
        std::net::IpAddr::V4(v4) => std::net::IpAddr::V4(std::net::Ipv4Addr::from(!v4.to_bits())),
        std::net::IpAddr::V6(v6) => std::net::IpAddr::V6(std::net::Ipv6Addr::from(!v6.to_bits())),
    };
    let result = cidr::IpInet::new(hostmask, inet.network_length())
        .expect("masklen valid for resolved inet");
    ValueRef::from(Value::inet_value(crate::value::InetValue::resolved(result))).into_raw(ctx)
}

/// PostgreSQL `network(inet) -> cidr`: the network portion (host
/// bits zeroed). Always produces a canonical cidr value.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_network(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("network() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "network", "addr");
    ValueRef::from(Value::cidr_value(crate::value::CidrValue::resolved(
        inet.network(),
    )))
    .into_raw(ctx)
}

/// PostgreSQL `set_masklen(inet, int) -> inet`: set the network
/// mask length of an inet to a new value, preserving the host
/// address.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_set_masklen(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("set_masklen() missing required argument 'addr'"));
    let masklen = get_call_arg(args, kwargs, 1, Some("masklen"))
        .unwrap_or_else(|| panic!("set_masklen() missing required argument 'masklen'"));
    let inet = arg_as_inet(&addr, "set_masklen", "addr");
    let new_mask = arg_as_int(&masklen, "set_masklen", "masklen");
    // Family-appropriate range. v4: 0..=32; v6: 0..=128.
    let max_mask = match inet {
        cidr::IpInet::V4(_) => 32,
        cidr::IpInet::V6(_) => 128,
    };
    if new_mask < 0 || new_mask > max_mask {
        panic!(
            "set_masklen() masklen {new_mask} out of range for v{} inet (expected 0..={max_mask})",
            if max_mask == 32 { "4" } else { "6" }
        );
    }
    let result =
        cidr::IpInet::new(inet.address(), new_mask as u8).expect("masklen bounds checked above");
    ValueRef::from(Value::inet_value(crate::value::InetValue::resolved(result))).into_raw(ctx)
}

/// PostgreSQL `text(inet) -> text`: canonical text form including
/// masklen.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_text(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("text() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "text", "addr");
    ValueRef::str(inet.to_string().as_ref()).into_raw(ctx)
}

/// PostgreSQL `abbrev(inet) -> text`: same as text() but suppresses
/// `/32` (v4) and `/128` (v6) when the inet is at host-mask
/// (effectively a single host).
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_abbrev(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("abbrev() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "abbrev", "addr");
    let suppress = matches!(
        (inet, inet.network_length()),
        (cidr::IpInet::V4(_), 32) | (cidr::IpInet::V6(_), 128)
    );
    let text = if suppress {
        inet.address().to_string()
    } else {
        inet.to_string()
    };
    ValueRef::str(text.as_ref()).into_raw(ctx)
}

/// Mokkan `family(inet) -> IpFamily`: typed enum return. Diverges
/// from PG which returns int 4/6 (per D6 / F1.4 PG-divergence
/// note); operators porting PG SQL rewrite
/// `family(addr) == 4` as `family(addr) == net.V4`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_family(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("family() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "family", "addr");
    let family = IpFamily::of_inet(&inet);
    ValueRef::from(Value::ip_family_value(family)).into_raw(ctx)
}

/// PostgreSQL `inet_merge(inet, inet) -> cidr`: smallest cidr
/// containing both networks. Family-mismatch is a runtime error
/// (PG's same).
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_inet_merge(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let a = get_call_arg(args, kwargs, 0, Some("a"))
        .unwrap_or_else(|| panic!("inet_merge() missing required argument 'a'"));
    let b = get_call_arg(args, kwargs, 1, Some("b"))
        .unwrap_or_else(|| panic!("inet_merge() missing required argument 'b'"));
    let a = arg_as_inet(&a, "inet_merge", "a");
    let b = arg_as_inet(&b, "inet_merge", "b");
    match (a, b) {
        (cidr::IpInet::V4(av), cidr::IpInet::V4(bv)) => {
            // Walk masks from current down to /0 until the two
            // addresses fall into the same network at mask N. To
            // avoid the cidr crate's strict-canonical
            // `Ipv4Cidr::new` rejecting non-network-aligned
            // inputs, mask the addresses ourselves and compare
            // the masked u32s directly.
            let a_bits = u32::from(av.address());
            let b_bits = u32::from(bv.address());
            let mut mask = av.network_length().min(bv.network_length());
            while mask > 0 {
                let m: u32 = if mask == 0 {
                    0
                } else {
                    !((1u32 << (32 - mask)) - 1)
                };
                if (a_bits & m) == (b_bits & m) {
                    let network = std::net::Ipv4Addr::from(a_bits & m);
                    let merged = cidr::IpCidr::V4(cidr::Ipv4Cidr::new(network, mask).unwrap());
                    return ValueRef::from(Value::cidr_value(crate::value::CidrValue::resolved(
                        merged,
                    )))
                    .into_raw(ctx);
                }
                mask -= 1;
            }
            // /0 always merges.
            let merged = cidr::IpCidr::V4(
                cidr::Ipv4Cidr::new(std::net::Ipv4Addr::new(0, 0, 0, 0), 0).unwrap(),
            );
            ValueRef::from(Value::cidr_value(crate::value::CidrValue::resolved(merged)))
                .into_raw(ctx)
        }
        (cidr::IpInet::V6(av), cidr::IpInet::V6(bv)) => {
            let a_bits = u128::from(av.address());
            let b_bits = u128::from(bv.address());
            let mut mask = av.network_length().min(bv.network_length());
            while mask > 0 {
                let m: u128 = if mask == 0 {
                    0
                } else {
                    !((1u128 << (128 - mask)) - 1)
                };
                if (a_bits & m) == (b_bits & m) {
                    let network = std::net::Ipv6Addr::from(a_bits & m);
                    let merged = cidr::IpCidr::V6(cidr::Ipv6Cidr::new(network, mask).unwrap());
                    return ValueRef::from(Value::cidr_value(crate::value::CidrValue::resolved(
                        merged,
                    )))
                    .into_raw(ctx);
                }
                mask -= 1;
            }
            let merged =
                cidr::IpCidr::V6(cidr::Ipv6Cidr::new(std::net::Ipv6Addr::UNSPECIFIED, 0).unwrap());
            ValueRef::from(Value::cidr_value(crate::value::CidrValue::resolved(merged)))
                .into_raw(ctx)
        }
        _ => panic!("inet_merge() operands must be the same family (v4-v4 or v6-v6)"),
    }
}

/// PostgreSQL `contains(cidr, cidr) -> bool`: does `outer` strictly
/// contain `inner` (proper subset)?
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_contains(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let outer = get_call_arg(args, kwargs, 0, Some("outer"))
        .unwrap_or_else(|| panic!("contains() missing required argument 'outer'"));
    let inner = get_call_arg(args, kwargs, 1, Some("inner"))
        .unwrap_or_else(|| panic!("contains() missing required argument 'inner'"));
    let outer = arg_as_cidr(&outer, "contains", "outer");
    let inner = arg_as_cidr(&inner, "contains", "inner");
    let result = cidr_strictly_contains(&outer, &inner);
    ValueRef::bool(result).into_raw(ctx)
}

/// PostgreSQL `contained_by(cidr, cidr) -> bool`: is `inner` a
/// proper subset of `outer`? (Inverse of `contains`.)
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_contained_by(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let inner = get_call_arg(args, kwargs, 0, Some("inner"))
        .unwrap_or_else(|| panic!("contained_by() missing required argument 'inner'"));
    let outer = get_call_arg(args, kwargs, 1, Some("outer"))
        .unwrap_or_else(|| panic!("contained_by() missing required argument 'outer'"));
    let inner = arg_as_cidr(&inner, "contained_by", "inner");
    let outer = arg_as_cidr(&outer, "contained_by", "outer");
    let result = cidr_strictly_contains(&outer, &inner);
    ValueRef::bool(result).into_raw(ctx)
}

/// PostgreSQL `contains_eq(cidr, cidr) -> bool`: outer contains
/// inner or equals it (non-strict containment).
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_contains_eq(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let outer = get_call_arg(args, kwargs, 0, Some("outer"))
        .unwrap_or_else(|| panic!("contains_eq() missing required argument 'outer'"));
    let inner = get_call_arg(args, kwargs, 1, Some("inner"))
        .unwrap_or_else(|| panic!("contains_eq() missing required argument 'inner'"));
    let outer = arg_as_cidr(&outer, "contains_eq", "outer");
    let inner = arg_as_cidr(&inner, "contains_eq", "inner");
    let result = cidr_contains_or_equals(&outer, &inner);
    ValueRef::bool(result).into_raw(ctx)
}

/// PostgreSQL `overlaps(cidr, cidr) -> bool`: do the two networks
/// share at least one address?
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_overlaps(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let a = get_call_arg(args, kwargs, 0, Some("a"))
        .unwrap_or_else(|| panic!("overlaps() missing required argument 'a'"));
    let b = get_call_arg(args, kwargs, 1, Some("b"))
        .unwrap_or_else(|| panic!("overlaps() missing required argument 'b'"));
    let a = arg_as_cidr(&a, "overlaps", "a");
    let b = arg_as_cidr(&b, "overlaps", "b");
    // Two networks overlap iff one contains the other (or they're
    // the same network). Equivalent to "either is contained_eq in
    // the other."
    let result = cidr_contains_or_equals(&a, &b) || cidr_contains_or_equals(&b, &a);
    ValueRef::bool(result).into_raw(ctx)
}

/// Internal: dispatch a unary `inet → bool` classifier to the
/// appropriate std-net predicate per family. Used by the F1.7+
/// classification predicates below; saves the same five-line
/// `arg_as_inet` + match dance from being repeated.
fn classify_inet<F4, F6>(inet: cidr::IpInet, v4: F4, v6: F6) -> bool
where
    F4: FnOnce(&std::net::Ipv4Addr) -> bool,
    F6: FnOnce(&std::net::Ipv6Addr) -> bool,
{
    match inet.address() {
        std::net::IpAddr::V4(a) => v4(&a),
        std::net::IpAddr::V6(a) => v6(&a),
    }
}

/// Mokkan `is_unspecified(inet) -> bool`: address is the
/// "unspecified" sentinel for its family — `0.0.0.0` (v4) or
/// `::` (v6). Replaces upstream KCL's stringly-typed
/// `is_unspecified_IP`. Thin wrapper over std-net.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_is_unspecified(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("is_unspecified() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "is_unspecified", "addr");
    let result = classify_inet(
        inet,
        std::net::Ipv4Addr::is_unspecified,
        std::net::Ipv6Addr::is_unspecified,
    );
    ValueRef::bool(result).into_raw(ctx)
}

/// Mokkan `is_loopback(inet) -> bool`: address falls in
/// `127.0.0.0/8` (v4) or equals `::1` (v6). Replaces upstream
/// KCL's `is_loopback_IP`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_is_loopback(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("is_loopback() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "is_loopback", "addr");
    let result = classify_inet(
        inet,
        std::net::Ipv4Addr::is_loopback,
        std::net::Ipv6Addr::is_loopback,
    );
    ValueRef::bool(result).into_raw(ctx)
}

/// Mokkan `is_multicast(inet) -> bool`: address falls in
/// `224.0.0.0/4` (v4) or `ff00::/8` (v6). Replaces upstream KCL's
/// `is_multicast_IP`. The narrower upstream variants
/// (`is_interface_local_multicast_IP`,
/// `is_link_local_multicast_IP`) are deferred — add when a
/// concrete homelab use case surfaces.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_is_multicast(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("is_multicast() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "is_multicast", "addr");
    let result = classify_inet(
        inet,
        std::net::Ipv4Addr::is_multicast,
        std::net::Ipv6Addr::is_multicast,
    );
    ValueRef::bool(result).into_raw(ctx)
}

/// Mokkan `is_link_local(inet) -> bool`: address falls in
/// `169.254.0.0/16` (v4) or `fe80::/10` (v6 unicast link-local).
/// Replaces upstream KCL's `is_link_local_unicast_IP` (the
/// "unicast" qualifier was redundant — link-local-multicast is
/// reachable via `is_multicast` if needed; the homelab use case
/// is "is this an APIPA / SLAAC autoconfig address?", which
/// link-local-unicast captures).
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_is_link_local(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("is_link_local() missing required argument 'addr'"));
    let inet = arg_as_inet(&addr, "is_link_local", "addr");
    let result = classify_inet(
        inet,
        std::net::Ipv4Addr::is_link_local,
        std::net::Ipv6Addr::is_unicast_link_local,
    );
    ValueRef::bool(result).into_raw(ctx)
}

/// Mokkan `inet_same_family(inet, inet) -> bool`: do both inets
/// have the same IP family?
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_inet_same_family(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let a = get_call_arg(args, kwargs, 0, Some("a"))
        .unwrap_or_else(|| panic!("inet_same_family() missing required argument 'a'"));
    let b = get_call_arg(args, kwargs, 1, Some("b"))
        .unwrap_or_else(|| panic!("inet_same_family() missing required argument 'b'"));
    let a = arg_as_inet(&a, "inet_same_family", "a");
    let b = arg_as_inet(&b, "inet_same_family", "b");
    let result = IpFamily::of_inet(&a) == IpFamily::of_inet(&b);
    ValueRef::bool(result).into_raw(ctx)
}

/// Helper: outer strictly contains inner (proper subset). Both
/// must be the same family; cross-family returns false.
fn cidr_strictly_contains(outer: &cidr::IpCidr, inner: &cidr::IpCidr) -> bool {
    match (outer, inner) {
        (cidr::IpCidr::V4(o), cidr::IpCidr::V4(i)) => {
            o.network_length() < i.network_length()
                && o.contains(&i.first_address())
                && o.contains(&i.last_address())
        }
        (cidr::IpCidr::V6(o), cidr::IpCidr::V6(i)) => {
            o.network_length() < i.network_length()
                && o.contains(&i.first_address())
                && o.contains(&i.last_address())
        }
        _ => false,
    }
}

/// Helper: outer contains inner or equals it.
fn cidr_contains_or_equals(outer: &cidr::IpCidr, inner: &cidr::IpCidr) -> bool {
    match (outer, inner) {
        (cidr::IpCidr::V4(o), cidr::IpCidr::V4(i)) => {
            o.network_length() <= i.network_length()
                && o.contains(&i.first_address())
                && o.contains(&i.last_address())
        }
        (cidr::IpCidr::V6(o), cidr::IpCidr::V6(i)) => {
            o.network_length() <= i.network_length()
                && o.contains(&i.first_address())
                && o.contains(&i.last_address())
        }
        _ => false,
    }
}

/// PostgreSQL `broadcast(inet)`: the broadcast address of an inet's
/// enclosing network. For v4: highest address in the inet's
/// `/N` (host bits all 1). For v6: same construction at the v6
/// scale. The returned `inet_value` carries the same masklen as
/// the input.
///
/// # Safety
/// This function involves raw pointer manipulation and should be
/// used with caution. Mirrors the contract of the existing
/// `kcl_net_split_host_port` and friends.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_broadcast(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    let addr = get_call_arg(args, kwargs, 0, Some("addr"))
        .unwrap_or_else(|| panic!("broadcast() missing required argument 'addr'"));

    let inet = arg_as_inet(&addr, "broadcast", "addr");

    // `cidr::IpInet::network()` produces a strict cidr; the
    // broadcast address is then the largest host within. The `cidr`
    // crate exposes `.last_address()` directly on `IpCidr` /
    // `Ipv4Cidr` / `Ipv6Cidr`. We construct the output inet with
    // the same masklen as the input so the output preserves the
    // network context.
    let network = inet.network();
    let broadcast = network.last_address();
    // Reconstruct an IpInet at the same network mask the source had.
    // (For v4, the broadcast address has host bits all 1 inside the
    // /N. For v6, "broadcast" is convention rather than protocol;
    // PG follows the same construction, we mirror that.)
    let result = cidr::IpInet::new(broadcast, inet.network_length())
        .expect("masklen valid for resolved inet");

    ValueRef::from(Value::inet_value(crate::value::InetValue::resolved(result))).into_raw(ctx)
}

// F1.7: stringly-typed survivors from upstream KCL's `net` package.
// Per F1.4.bis: the typed inet algebra surface above replaces the
// CIDR_*/parse_*/is_*/IP_*/to_* upstream functions (operators write
// `addr: inet = "1.2.3.4/24"` and the F1.3 string-coercion handles
// it; predicates like `contains()` / `overlaps()` cover what
// `is_IP_in_CIDR()` did). Three string-shaped utility functions
// have no typed equivalent yet, so they survive under `mokkan.net`
// with their existing signatures until a typed redesign emerges:
//
//   * `fqdn(name?: str) -> str` — DNS lookup; orthogonal to inet
//     types entirely.
//   * `split_host_port(ip_end_point: str) -> [str]` — parses
//     `host:port` / `[v6]:port`; ports aren't a typed concept here.
//   * `join_host_port(host, port: int | str) -> str` — inverse of
//     the above.

/// Mokkan `split_host_port(ip_end_point: str) -> [str]`. Survives
/// from upstream KCL `net.split_host_port` (D6 / F1.4.bis); same
/// stringly-typed signature, panic-on-malformed-input behaviour.
/// Implementation copied verbatim from the old upstream surface.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_split_host_port(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    if let Some(ip_end_point) = get_call_arg(args, kwargs, 0, Some("ip_end_point")) {
        let ip_end_point_str = ip_end_point.as_str();
        match ip_end_point_str.rsplit_once(':') {
            None => panic!(
                "ip_end_point \"{}\" missing port",
                ip_end_point_str.escape_default()
            ),
            Some((host, port)) => {
                if host.starts_with('[') {
                    match ip_end_point_str.find(']') {
                        None => panic!(
                            "ip_end_point \"{}\" missing ']'",
                            ip_end_point_str.escape_default()
                        ),
                        Some(end) => {
                            if end > host.len() || !ip_end_point_str[end + 1..].starts_with(':') {
                                panic!(
                                    "ip_end_point \"{}\" missing port",
                                    ip_end_point_str.escape_default()
                                );
                            }
                            if end < host.len() - 1 {
                                panic!(
                                    "ip_end_point \"{}\" too many colons",
                                    ip_end_point_str.escape_default()
                                );
                            }
                            if ip_end_point_str[1..].contains('[') {
                                panic!(
                                    "ip_end_point \"{}\" unexpected '['",
                                    ip_end_point_str.escape_default()
                                );
                            }
                            if port.contains(']') {
                                panic!(
                                    "ip_end_point \"{}\" unexpected ']'",
                                    ip_end_point_str.escape_default()
                                );
                            }
                            return ValueRef::list(Some(&[
                                &ValueRef::str(&host[1..end]),
                                &ValueRef::str(port),
                            ]))
                            .into_raw(ctx);
                        }
                    }
                }
                if host.contains(':') {
                    panic!(
                        "ip_end_point \"{}\" too many colons",
                        ip_end_point_str.escape_default()
                    );
                }
                if ip_end_point_str[1..].contains('[') {
                    panic!(
                        "ip_end_point \"{}\" unexpected '['",
                        ip_end_point_str.escape_default()
                    );
                }
                if ip_end_point_str.contains(']') {
                    panic!(
                        "ip_end_point \"{}\" unexpected ']'",
                        ip_end_point_str.escape_default()
                    );
                }
                return ValueRef::list(Some(&[&ValueRef::str(host), &ValueRef::str(port)]))
                    .into_raw(ctx);
            }
        }
    }

    panic!("split_host_port() missing 1 required positional argument: 'ip_end_point'");
}

/// Mokkan `join_host_port(host, port) -> str`. Inverse of
/// `split_host_port`. Survives from upstream KCL with the same
/// signature.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_join_host_port(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let ctx = unsafe { mut_ptr_as_ref(ctx) };

    if let Some(host) = get_call_arg(args, kwargs, 0, Some("host"))
        && let Some(port) = get_call_arg(args, kwargs, 1, Some("port"))
    {
        if host.as_str().contains(':') {
            return ValueRef::str(format!("[{host}]:{port}").as_ref()).into_raw(ctx);
        }
        return ValueRef::str(format!("{host}:{port}").as_ref()).into_raw(ctx);
    }
    panic!("join_host_port() missing 2 required positional arguments: 'host' and 'port'");
}

/// Mokkan `fqdn(name?: str) -> str`. Survives from upstream KCL.
/// DNS lookup; orthogonal to inet types. Native-target uses
/// `hostname` + `dns_lookup`; wasm target panics (no syscalls).
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_fqdn(
    ctx: *mut kcl_context_t,
    args: *const kcl_value_ref_t,
    kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    use std::net::ToSocketAddrs;
    let ctx = unsafe { mut_ptr_as_ref(ctx) };
    let args = unsafe { ptr_as_ref(args) };
    let kwargs = unsafe { ptr_as_ref(kwargs) };
    let name = get_call_arg_str(args, kwargs, 0, Some("name")).unwrap_or_default();
    let hostname = if name.is_empty() {
        match hostname::get() {
            Ok(name) => name.to_string_lossy().into_owned(),
            Err(_) => return ValueRef::str("").into_raw(ctx),
        }
    } else {
        name
    };
    match (hostname.as_str(), 0).to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                match dns_lookup::lookup_addr(&addr.ip()) {
                    Ok(fqdn) => ValueRef::str(&fqdn),
                    Err(_) => ValueRef::str(&hostname),
                }
            } else {
                ValueRef::str(&hostname)
            }
        }
        Err(_) => ValueRef::str(&hostname),
    }
    .into_raw(ctx)
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn kcl_mokkan_net_fqdn(
    _ctx: *mut kcl_context_t,
    _args: *const kcl_value_ref_t,
    _kwargs: *const kcl_value_ref_t,
) -> *const kcl_value_ref_t {
    panic!("fqdn() does not support the WASM target");
}
