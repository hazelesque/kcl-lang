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
