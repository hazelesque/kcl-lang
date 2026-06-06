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
        Value::inet_value(i) => *i,
        _ => panic!(
            "{func}() expected inet for argument '{arg}', got {} ({})",
            value.type_str(),
            value,
        ),
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

    ValueRef::from(Value::inet_value(result)).into_raw(ctx)
}
