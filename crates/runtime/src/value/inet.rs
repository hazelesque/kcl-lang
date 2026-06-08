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
    /// The handle-shaped leaf: a reference to a named symbolic subnet
    /// that the resolver will substitute at resolve time. The
    /// resolver looks up size + family from the subnets side-table
    /// against the same handle.
    ///
    /// Phase C.4 collapsed this leaf per Rev 6 D3: Rev 5 carried
    /// `size: Option<u8>` and `family: Option<IpFamily>` so the
    /// operator could assert them at the call site for validation.
    /// Rev 6 drops the assertion path entirely — the side-table is
    /// the single source of truth, and the operator never needs to
    /// restate values that already live in the `subnets = {...}`
    /// declaration. The cost of the collapse: typo-at-call-site
    /// catches go away; the benefit: the IR is simpler and
    /// resolver eval has fewer branches.
    HandleSubnet { subnet_handle: String },

    /// VIP-shaped leaf: a reference to a named `NetworkAddress`
    /// (Phase D consumer). Resolver looks up the IPAM-allocated
    /// inet for the VIP at resolve time.
    ///
    /// Per Rev 6 D3-relaxed: a VIP is categorically distinct from
    /// a subnet (no `size`, no `family`; it's just a key into the
    /// IPAM *address* table, parent CIDR implicit). Composition
    /// like `BroadcastOf(HandleAddress{...})` is semantically
    /// nonsense — a VIP isn't a subnet — but the IR allows it; the
    /// resolver surfaces a type error if the operator manages to
    /// write such a composition.
    ///
    /// Phase C.4 adds the leaf shape. The synthesis API
    /// (`kcl_embed::resolve::pending_address`) constructs values
    /// carrying this leaf. The resolver's eval-path lights up in
    /// Phase D when VIP allocation actually runs; for now the
    /// Phase A→D loud guard at config-load time prevents VIP
    /// declarations from reaching the resolver.
    HandleAddress { vip_handle: String },

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
                "expected resolved cidr but got symbolic — D2 forbids deferred predicates \
                 (no <, ==, contains, etc. on symbolic operands), and schema fields that \
                 should accept symbolic values must be declared as `cidr | ResolvableString` \
                 to flow through the F2.7 codegen `Resolvable<T>` shape"
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
                "expected resolved inet but got symbolic — D2 forbids deferred predicates \
                 (no <, ==, contains, etc. on symbolic operands), and schema fields that \
                 should accept symbolic values must be declared as `inet | ResolvableString` \
                 to flow through the F2.7 codegen `Resolvable<T>` shape"
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

// ============================================================================
// F2.3: ResolvableString — deferred-string value carrier.
// ============================================================================
//
// `ResolvableString` is the value-level container for stringification
// of symbolic values. F2.4 will use it as the result of `text()` /
// `host()` / `abbrev()` / `masklen()` (stringified) on symbolic
// inputs, and as the result of string concatenation when either
// operand is itself a symbolic-derived stringification.
//
// The shape is a flat segment list — each segment is either a
// resolved string fragment (`Literal`) or a deferred symbolic
// expression that the resolver will substitute at resolve time
// (`Symbolic`). Empty segment list = empty resolved string. A
// single-`Literal` list = an eagerly-resolvable string (same
// downstream behaviour as a bare `str_value`); the segment
// representation just keeps the type uniform.
//
// Generic name (`ResolvableString`, not `ResolvableInetString`) is
// honest per plan: although today only inet-shaped operations
// produce one, the type doesn't lock that in. Any future
// symbolic-derived stringification (`mokkan.symbolic` direct
// constructors, secret-management deferral, monorepo-config
// cross-references) gets the same carrier.
//
// F2.3 ships only the type shape + value variant. F2.4 wires the
// stringification / concat dispatch.

/// One segment of a `ResolvableString`. Either a resolved string
/// fragment or a deferred symbolic expression carrying an `Expr` IR
/// for later resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Resolved string fragment, used verbatim at resolution time.
    Literal(String),
    /// Symbolic IR tree. The resolver evaluates this and stringifies
    /// the result. Box keeps the Segment enum's discriminant size
    /// manageable — Expr is recursive.
    Symbolic(Box<Expr>),
}

/// Value-level container for a deferred string. See module-level
/// docs for the shape rationale.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResolvableString {
    pub segments: Vec<Segment>,
}

impl ResolvableString {
    /// Construct an empty `ResolvableString`. Equivalent to the empty
    /// resolved string after resolution.
    pub fn new() -> Box<Self> {
        Box::new(ResolvableString {
            segments: Vec::new(),
        })
    }

    /// Construct a `ResolvableString` from a single resolved
    /// fragment. Equivalent to a bare `str_value` after resolution
    /// but keeps the type uniform with the symbolic-flow case.
    pub fn from_literal(s: impl Into<String>) -> Box<Self> {
        Box::new(ResolvableString {
            segments: vec![Segment::Literal(s.into())],
        })
    }

    /// Construct a `ResolvableString` from a single symbolic
    /// expression. Used by F2.4's stringification dispatch on
    /// symbolic operands (`text(symbolic_inet)` etc.).
    pub fn from_symbolic(expr: Expr) -> Box<Self> {
        Box::new(ResolvableString {
            segments: vec![Segment::Symbolic(Box::new(expr))],
        })
    }

    /// Construct from a pre-built segment list.
    pub fn from_segments(segments: Vec<Segment>) -> Box<Self> {
        Box::new(ResolvableString { segments })
    }

    /// Whether every segment is `Literal` — the value is eagerly
    /// resolvable without consulting the resolver. Empty segment list
    /// is trivially "fully literal".
    pub fn is_fully_literal(&self) -> bool {
        self.segments
            .iter()
            .all(|s| matches!(s, Segment::Literal(_)))
    }

    /// Whether any segment is `Symbolic` — the value requires
    /// resolution before consumption.
    pub fn has_symbolic(&self) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s, Segment::Symbolic(_)))
    }
}

impl Default for ResolvableString {
    fn default() -> Self {
        ResolvableString {
            segments: Vec::new(),
        }
    }
}

impl std::fmt::Display for ResolvableString {
    /// Canonical text form: concatenated Literal segments, with
    /// Symbolic segments rendered as `${<expr-debug>}`. This is for
    /// diagnostics — the operator-facing resolved string flows
    /// through the resolver pass (F2.4/T4), not this Display impl.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => f.write_str(s)?,
                Segment::Symbolic(e) => write!(f, "${{{e:?}}}")?,
            }
        }
        Ok(())
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
            subnet_handle: HANDLE.to_string(),
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
    #[should_panic(expected = "D2 forbids deferred predicates")]
    fn cidr_value_expect_resolved_panics_on_symbolic() {
        let cv = CidrValue::symbolic(Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
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
                subnet_handle: HANDLE.to_string(),
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

    // F2.3: ResolvableString shape tests.
    // ────────────────────────────────────────────────────────────

    const LITERAL_PREFIX: &str = "host-";
    const LITERAL_SUFFIX: &str = "-fin";

    #[test]
    fn resolvable_string_empty_is_fully_literal_and_no_symbolic() {
        let rs = ResolvableString::new();
        assert!(rs.is_fully_literal(), "empty list is trivially literal");
        assert!(!rs.has_symbolic(), "empty list carries no symbolic");
        assert_eq!(format!("{rs}"), "", "Display of empty is empty string");
    }

    #[test]
    fn resolvable_string_from_literal_is_fully_literal() {
        let rs = ResolvableString::from_literal(LITERAL_PREFIX);
        assert!(rs.is_fully_literal());
        assert!(!rs.has_symbolic());
        assert_eq!(format!("{rs}"), LITERAL_PREFIX);
    }

    #[test]
    fn resolvable_string_from_symbolic_has_symbolic_not_literal() {
        let e = Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        };
        let rs = ResolvableString::from_symbolic(e);
        assert!(!rs.is_fully_literal());
        assert!(rs.has_symbolic());
        let display = format!("{rs}");
        assert!(
            display.starts_with("${") && display.ends_with("}"),
            "symbolic Display should wrap in ${{...}}, got: {display}"
        );
    }

    /// Multi-segment "prefix-${handle}-suffix" shape — the canonical
    /// concat target. Display concatenates literals around the
    /// debug-shaped symbolic placeholder.
    #[test]
    fn resolvable_string_multi_segment_concat_display() {
        let e = Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        };
        let rs = ResolvableString::from_segments(vec![
            Segment::Literal(LITERAL_PREFIX.to_string()),
            Segment::Symbolic(Box::new(e)),
            Segment::Literal(LITERAL_SUFFIX.to_string()),
        ]);
        assert!(!rs.is_fully_literal());
        assert!(rs.has_symbolic());
        let display = format!("{rs}");
        assert!(
            display.starts_with(LITERAL_PREFIX) && display.ends_with(LITERAL_SUFFIX),
            "literal fragments should bracket the symbolic placeholder, got: {display}"
        );
    }

    /// ResolvableString derives Eq + Hash — both used by F2.7
    /// codegen-side dedup and resolver caches.
    #[test]
    fn resolvable_string_round_trips_through_clone_and_eq() {
        let e = Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        };
        let a = *ResolvableString::from_segments(vec![
            Segment::Literal(LITERAL_PREFIX.to_string()),
            Segment::Symbolic(Box::new(e)),
        ]);
        let b = a.clone();
        assert_eq!(a, b);
        let mut s = std::collections::HashSet::new();
        s.insert(a);
        s.insert(b);
        assert_eq!(
            s.len(),
            1,
            "HashSet should dedup identical resolvable strings"
        );
    }
}
