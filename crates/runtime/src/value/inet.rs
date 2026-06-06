//! Mokkan native network value types.
//!
//! F1 ships the *resolved* state: a `cidr::IpCidr` / `cidr::IpInet` /
//! `macaddr::MacAddr6` / `macaddr::MacAddr8` carried by name. Plus
//! `IpFamily`, a tagged enum used everywhere the IP family needs
//! explicit declaration (network definitions, symbolic-subnet builtin
//! signatures, IPAM allocator). Never defaulted — operators declare
//! family explicitly so v4 is not silently baked in as a forever
//! assumption.
//!
//! F2 (a later stage) wraps `IpCidr` / `IpInet` in an enum carrying
//! either the resolved value or a symbolic IR tree for deferred
//! resolution. F1 just lands the typed carriers.

/// IP address family. Used by `NetworkDefinition.family`, the
/// `symbolic_subnet` builtin signature, the IPAM allocator API, and
/// the resolver's family validation. The variants are exposed to
/// mokkan code as `net.V4` / `net.V6` constants on the `mokkan.net`
/// package (registered alongside the named type in sema).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IpFamily {
    V4,
    V6,
}

impl IpFamily {
    /// `4` or `6`. Returned as `i64` to match mokkan's int width.
    /// PostgreSQL convention names this `family(addr)`; we differ
    /// from PG by returning a typed enum (this) rather than a magic
    /// int, and provide the i64 form as a deliberate
    /// `family_as_int()` for callers that need it (cloud-init
    /// templates, netplan keys, etc.).
    pub fn as_int(self) -> i64 {
        match self {
            IpFamily::V4 => 4,
            IpFamily::V6 => 6,
        }
    }

    /// Family of a `cidr::IpCidr`.
    pub fn of_cidr(c: &cidr::IpCidr) -> Self {
        match c {
            cidr::IpCidr::V4(_) => IpFamily::V4,
            cidr::IpCidr::V6(_) => IpFamily::V6,
        }
    }

    /// Family of a `cidr::IpInet`.
    pub fn of_inet(i: &cidr::IpInet) -> Self {
        match i {
            cidr::IpInet::V4(_) => IpFamily::V4,
            cidr::IpInet::V6(_) => IpFamily::V6,
        }
    }
}

impl std::fmt::Display for IpFamily {
    /// Canonical textual form. `"V4"` / `"V6"`. Matches the constant
    /// name operators write in mokkan source (`net.V4` / `net.V6`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpFamily::V4 => f.write_str("V4"),
            IpFamily::V6 => f.write_str("V6"),
        }
    }
}

/// `inet + int` / `inet - int` (offset arithmetic). Adds `offset` to
/// the address portion of `inet`, preserving the masklen. Overflow
/// (v4 going outside `0..=u32::MAX`, or v6 outside `0..=u128::MAX`)
/// panics with a clear message. PostgreSQL's silent wrap is not
/// inherited.
pub fn inet_add_offset(inet: cidr::IpInet, offset: i64) -> cidr::IpInet {
    let masklen = inet.network_length();
    match inet {
        cidr::IpInet::V4(v4) => {
            let bits = i64::from(u32::from(v4.address()));
            let new = bits.checked_add(offset).unwrap_or_else(|| {
                panic!("inet arithmetic overflow: v4 address {bits} + {offset} overflows i64")
            });
            if !(0..=i64::from(u32::MAX)).contains(&new) {
                panic!(
                    "inet arithmetic overflow: v4 address {bits} + {offset} = {new} out of range"
                );
            }
            let addr = std::net::Ipv4Addr::from(new as u32);
            cidr::IpInet::new(std::net::IpAddr::V4(addr), masklen)
                .expect("masklen preserved from source inet")
        }
        cidr::IpInet::V6(v6) => {
            let bits = u128::from(v6.address());
            let new = if offset >= 0 {
                bits.checked_add(offset as u128)
            } else {
                bits.checked_sub(offset.unsigned_abs() as u128)
            }
            .unwrap_or_else(|| {
                panic!("inet arithmetic overflow: v6 address {bits} + {offset} out of range")
            });
            let addr = std::net::Ipv6Addr::from(new);
            cidr::IpInet::new(std::net::IpAddr::V6(addr), masklen)
                .expect("masklen preserved from source inet")
        }
    }
}

/// `inet - inet` (signed distance). v4-v4 always fits `i64` (max
/// span `u32::MAX`). v6-v6 may overflow — distance up to `2^128`;
/// any absolute value above `i64::MAX` panics with a clear
/// "v6 distance exceeds i64" message. Cross-family panics.
pub fn inet_distance(a: cidr::IpInet, b: cidr::IpInet) -> i64 {
    match (a, b) {
        (cidr::IpInet::V4(av), cidr::IpInet::V4(bv)) => {
            i64::from(u32::from(av.address())) - i64::from(u32::from(bv.address()))
        }
        (cidr::IpInet::V6(av), cidr::IpInet::V6(bv)) => {
            let a_bits = u128::from(av.address());
            let b_bits = u128::from(bv.address());
            if a_bits >= b_bits {
                let d = a_bits - b_bits;
                if d > i64::MAX as u128 {
                    panic!("inet arithmetic overflow: v6 distance {d} exceeds i64");
                }
                d as i64
            } else {
                let d = b_bits - a_bits;
                if d > i64::MAX as u128 {
                    panic!("inet arithmetic overflow: v6 distance {d} exceeds i64");
                }
                -(d as i64)
            }
        }
        _ => panic!("inet - inet: operands must be the same family (v4-v4 or v6-v6)"),
    }
}
