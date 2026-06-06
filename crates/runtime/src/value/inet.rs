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

// ============================================================================
// F2.1: dual-state newtypes + Expr IR for symbolic-mode cidr / inet values.
// ============================================================================
//
// F1 shipped `cidr_value(IpCidr)` / `inet_value(IpInet)` as bare typed
// payloads. F2 wraps those in newtype structs whose inner enum carries
// either the resolved value (the F1 case, unchanged) or a symbolic IR
// tree (`Expr`) for deferred resolution.
//
// The symbolic case is what makes the IPAM story land: a Tilleyfile can
// reference a network handle (`net_symbolic.symbolic_subnet("lan", 24, V4)`)
// before the host-side IPAM allocator has picked a concrete /24 for it,
// and downstream derivations (`net.broadcast(handle)`, etc.) compose
// the IR rather than evaluating eagerly. The resolver pass later walks
// the IR against the IPAM allocation map and substitutes resolved
// values.
//
// F2.1 lands ONLY the type shape — every existing F1 algebra function
// stays eager-only and panics if given a symbolic operand. F2.2 wires
// the symbolic-dispatch arms.

/// Symbolic IR tree for deferred cidr / inet computation. The shape is
/// load-bearing — its variants are the strict subset of operations the
/// D4 propagation matrix marks as flow-through-able to symbolic state.
///
/// Predicates (`contains`, `overlaps`, `==`, `<`, …) are deliberately
/// absent: D2 says symbolic mode is a *value-derivation* facility, not
/// a value-decision facility. Producing those at runtime against
/// symbolic operands is a hard error, not a deferred bool — and the
/// IR's structural shape enforces that by simply not having a node
/// for it.
///
/// `Family(Box<Expr>)` is also deliberately absent — `family()` on a
/// symbolic value is a metadata query, not a derivation, and the
/// operator already has the family eagerly available via the
/// network's `NetworkDefinition.family` declaration. See D4 notes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Expr {
    // ---- Leaves ---------------------------------------------------
    /// The handle-shaped leaf: a reference to a named symbolic network
    /// that the resolver will substitute at resolve time. `size` and
    /// `family` are `Option<T>` so the IR can honestly distinguish
    /// "operator asserted this value at the call site" (Some, resolver
    /// validates against the network's declaration) from "operator
    /// asserted nothing; inherit from the network" (None, resolver
    /// looks it up without validation).
    HandleSubnet {
        handle: String,
        size: Option<u8>,
        family: Option<IpFamily>,
    },

    /// Concrete cidr literal — produced when an eager value flows into
    /// an otherwise-symbolic expression (e.g.,
    /// `set_masklen(symbolic_subnet, 16)` where 16 is eager).
    LiteralCidr(cidr::IpCidr),
    /// Concrete inet literal — same shape as `LiteralCidr` but for
    /// inet derivations.
    LiteralInet(cidr::IpInet),
    /// Concrete int literal — masklen / offset / etc. arguments to
    /// symbolic operations.
    LiteralInt(i64),

    // ---- Derivations producing cidr ------------------------------
    /// `network(inet)`: the network portion (host bits zeroed) as a
    /// strict cidr.
    NetworkOf(Box<Expr>),
    /// `inet_merge(inet, inet)`: smallest cidr containing both.
    /// Resolver enforces family match at evaluation time.
    InetMerge(Box<Expr>, Box<Expr>),
    /// `set_masklen(inet, int)`: result is a cidr at the new mask
    /// length. Invalidates the source handle's `size` annotation per
    /// D4 — the resolver re-derives size from the SetMasklen literal.
    SetMasklen(Box<Expr>, Box<Expr>),

    // ---- Derivations producing inet ------------------------------
    /// `broadcast(inet)`: the broadcast address of an inet's enclosing
    /// network.
    BroadcastOf(Box<Expr>),
    /// `host(inet)`: the host address (no masklen) as an inet — but
    /// note `host()` is also used as the stringification primitive at
    /// the boundary; F2.4 emits this as part of a `Text` in some
    /// paths.
    HostOf(Box<Expr>),
    /// `netmask(inet)`: the netmask as an inet.
    NetmaskOf(Box<Expr>),
    /// `hostmask(inet)`: the host mask (bitwise complement of netmask).
    HostmaskOf(Box<Expr>),
    /// `inet + int`: offset arithmetic, masklen preserved.
    AddOffset(Box<Expr>, Box<Expr>),
    /// `inet - int`: offset arithmetic; mirror of AddOffset.
    SubOffset(Box<Expr>, Box<Expr>),

    // ---- Derivations producing int -------------------------------
    /// `masklen(inet)`: the network mask length.
    MaskLen(Box<Expr>),

    // ---- Stringification ----------------------------------------
    /// `text(inet)`: canonical text form including masklen.
    Text(Box<Expr>),
    /// `host(inet)`: text form of the address without the masklen.
    Host(Box<Expr>),
    /// `abbrev(inet)`: text form with `/32` (v4) and `/128` (v6)
    /// suppressed at host-mask.
    Abbrev(Box<Expr>),
}

/// Runtime newtype wrapping a cidr value. F2 dual-state: either the
/// resolved cidr (the F1 case, unchanged) or a symbolic IR tree
/// (`Expr`) for deferred resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CidrValue {
    pub inner: CidrInner,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CidrInner {
    Resolved(cidr::IpCidr),
    /// Symbolic IR — `Expr` must evaluate to a cidr at resolve time.
    /// The runtime never inspects the Expr's shape here; type
    /// consistency is the resolver's job.
    Symbolic(Expr),
}

impl CidrValue {
    /// Construct a resolved cidr value (the F1 shape). Convenience for
    /// `Box<CidrValue> { CidrInner::Resolved(c) }` at every site that
    /// used to write bare `Value::cidr_value(c)`.
    pub fn resolved(c: cidr::IpCidr) -> Box<Self> {
        Box::new(CidrValue {
            inner: CidrInner::Resolved(c),
        })
    }

    /// Construct a symbolic cidr value carrying an IR tree. Used by
    /// `net_symbolic.symbolic_subnet` and any algebra function whose
    /// inputs included a symbolic operand.
    pub fn symbolic(expr: Expr) -> Box<Self> {
        Box::new(CidrValue {
            inner: CidrInner::Symbolic(expr),
        })
    }

    /// Whether this cidr is in the resolved state.
    pub fn is_resolved(&self) -> bool {
        matches!(self.inner, CidrInner::Resolved(_))
    }

    /// Whether this cidr is in the symbolic state.
    pub fn is_symbolic(&self) -> bool {
        matches!(self.inner, CidrInner::Symbolic(_))
    }

    /// Extract the resolved cidr; panic if symbolic. Used by F2.1 call
    /// sites that haven't been ported to symbolic dispatch yet — those
    /// land in F2.2 and the panic disappears as each algebra function
    /// gains its symbolic arm.
    pub fn expect_resolved(&self) -> cidr::IpCidr {
        match &self.inner {
            CidrInner::Resolved(c) => *c,
            CidrInner::Symbolic(_) => panic!(
                "expected resolved cidr but got symbolic — \
                 the F2.2 symbolic-algebra dispatch wires this surface"
            ),
        }
    }
}

impl std::fmt::Display for CidrValue {
    /// Canonical text form: resolved cidrs format as their underlying
    /// `IpCidr` (e.g., `"10.0.0.0/24"`); symbolic cidrs format as a
    /// debug-shaped IR snippet. The symbolic Display is for diagnostics
    /// — operator-facing stringification flows through F2.4's
    /// `ResolvableString` path, not this.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            CidrInner::Resolved(c) => write!(f, "{c}"),
            CidrInner::Symbolic(e) => write!(f, "<symbolic cidr: {e:?}>"),
        }
    }
}

impl From<cidr::IpCidr> for Box<CidrValue> {
    fn from(c: cidr::IpCidr) -> Self {
        CidrValue::resolved(c)
    }
}

/// Runtime newtype wrapping an inet value. Same dual-state shape as
/// `CidrValue` — see its docs for the semantics.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InetValue {
    pub inner: InetInner,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InetInner {
    Resolved(cidr::IpInet),
    /// Symbolic IR — `Expr` must evaluate to an inet at resolve time.
    Symbolic(Expr),
}

impl InetValue {
    pub fn resolved(i: cidr::IpInet) -> Box<Self> {
        Box::new(InetValue {
            inner: InetInner::Resolved(i),
        })
    }
    pub fn symbolic(expr: Expr) -> Box<Self> {
        Box::new(InetValue {
            inner: InetInner::Symbolic(expr),
        })
    }
    pub fn is_resolved(&self) -> bool {
        matches!(self.inner, InetInner::Resolved(_))
    }
    pub fn is_symbolic(&self) -> bool {
        matches!(self.inner, InetInner::Symbolic(_))
    }
    /// Extract the resolved inet; panic if symbolic. Same shape and
    /// rationale as `CidrValue::expect_resolved` — see its docs.
    pub fn expect_resolved(&self) -> cidr::IpInet {
        match &self.inner {
            InetInner::Resolved(i) => *i,
            InetInner::Symbolic(_) => panic!(
                "expected resolved inet but got symbolic — \
                 the F2.2 symbolic-algebra dispatch wires this surface"
            ),
        }
    }
}

impl std::fmt::Display for InetValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            InetInner::Resolved(i) => write!(f, "{i}"),
            InetInner::Symbolic(e) => write!(f, "<symbolic inet: {e:?}>"),
        }
    }
}

impl From<cidr::IpInet> for Box<InetValue> {
    fn from(i: cidr::IpInet) -> Self {
        InetValue::resolved(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // String literals shared between setup + expectation so an
    // accidental edit to one and not the other surfaces as a test
    // failure rather than a silent type-mismatch we never notice.
    const CSTR: &str = "10.0.0.0/24";
    const ISTR: &str = "10.0.0.10/24";
    const HANDLE: &str = "lan";

    #[test]
    fn cidr_value_resolved_round_trip() {
        let c = cidr::IpCidr::from_str(CSTR).unwrap();
        let cv = CidrValue::resolved(c);
        assert!(cv.is_resolved());
        assert!(!cv.is_symbolic());
        assert_eq!(cv.expect_resolved(), c);
        assert_eq!(format!("{cv}"), CSTR);
    }

    #[test]
    fn inet_value_resolved_round_trip() {
        let i = cidr::IpInet::from_str(ISTR).unwrap();
        let iv = InetValue::resolved(i);
        assert!(iv.is_resolved());
        assert!(!iv.is_symbolic());
        assert_eq!(iv.expect_resolved(), i);
        assert_eq!(format!("{iv}"), ISTR);
    }

    #[test]
    fn cidr_value_symbolic_carries_expr() {
        let e = Expr::HandleSubnet {
            handle: HANDLE.to_string(),
            size: Some(24),
            family: Some(IpFamily::V4),
        };
        let cv = CidrValue::symbolic(e.clone());
        assert!(cv.is_symbolic());
        assert!(!cv.is_resolved());
        // Symbolic Display is for diagnostics only — exact format is
        // a debug-shaped snippet; lock down only the prefix so the
        // assertion survives if we refine the format later.
        assert!(
            format!("{cv}").starts_with("<symbolic cidr: "),
            "diag-format unexpected: {cv}"
        );
    }

    #[test]
    #[should_panic(expected = "expected resolved cidr but got symbolic")]
    fn cidr_value_expect_resolved_panics_on_symbolic() {
        let cv = CidrValue::symbolic(Expr::HandleSubnet {
            handle: HANDLE.to_string(),
            size: None,
            family: None,
        });
        let _ = cv.expect_resolved();
    }

    #[test]
    fn from_ipcidr_for_boxed_cidr_value_gives_resolved() {
        let c = cidr::IpCidr::from_str(CSTR).unwrap();
        let cv: Box<CidrValue> = c.into();
        assert!(cv.is_resolved());
        assert_eq!(cv.expect_resolved(), c);
    }

    #[test]
    fn expr_handle_subnet_round_trips_through_clone_and_eq() {
        // F2.1 derives Clone/PartialEq/Hash on Expr — tests downstream
        // (resolver IR construction, IR diffing, hashed handle
        // caches) rely on all three. Lock them in.
        let a = Expr::AddOffset(
            Box::new(Expr::NetworkOf(Box::new(Expr::HandleSubnet {
                handle: HANDLE.to_string(),
                size: Some(24),
                family: Some(IpFamily::V4),
            }))),
            Box::new(Expr::LiteralInt(10)),
        );
        let b = a.clone();
        assert_eq!(a, b);
        // Hash equality follows from Eq for derived impls; just touch
        // it to make sure the trait bound resolves.
        let mut s = std::collections::HashSet::new();
        s.insert(a);
        s.insert(b);
        assert_eq!(s.len(), 1);
    }
}
