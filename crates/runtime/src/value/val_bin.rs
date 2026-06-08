//! Copyright The KCL Authors. All rights reserved.

use crate::*;

impl ValueRef {
    pub fn bin_add(&self, ctx: &mut Context, x: &Self) -> Self {
        let strict_range_check_32 = ctx.cfg.strict_range_check;
        let strict_range_check_64 = ctx.cfg.debug_mode || !ctx.cfg.strict_range_check;

        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_i32_overflow_add(*a, *b) {
                    panic_i32_overflow!(ctx, *a as i128 + *b as i128);
                }
                if strict_range_check_64 && is_i64_overflow_add(*a, *b) {
                    panic_i64_overflow!(ctx, *a as i128 + *b as i128);
                }

                Self::int(*a + *b)
            }
            (Value::float_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_add(*a, *b) {
                    panic_f32_overflow!(ctx, *a + *b);
                }
                Self::float(*a + *b)
            }
            (Value::int_value(a), Value::float_value(b)) => {
                if is_f32_overflow_add(*a as f64, *b) {
                    panic_f32_overflow!(ctx, *a as f64 + *b);
                }
                Self::float(*a as f64 + *b)
            }
            (Value::float_value(a), Value::int_value(b)) => {
                if is_f32_overflow_add(*a, *b as f64) {
                    panic_f32_overflow!(ctx, *a + *b as f64);
                }
                Self::float(*a + *b as f64)
            }

            (Value::str_value(a), Value::str_value(b)) => {
                Self::str(format!("{}{}", *a, *b).as_ref())
            }
            // F2.4: string concat with a ResolvableString operand
            // produces a ResolvableString with the concatenated
            // segment list. The string operand becomes a single
            // Literal segment; existing ResolvableString segments
            // are spliced in directly. ResolvableString + str:
            // segments + Literal. str + ResolvableString: Literal +
            // segments. RS + RS: segments + segments. The plan
            // explicitly defers `str + inet/cidr` (resolved or
            // symbolic) to operators using explicit
            // `text(addr)` / `host(addr)` / `abbrev(addr)` — the
            // implicit stringification rule (text? host? abbrev?)
            // would surprise the operator more than the missing
            // arm does.
            (Value::str_value(a), Value::resolvable_string_value(b)) => {
                let mut segs = Vec::with_capacity(b.segments.len() + 1);
                segs.push(crate::value::Segment::Literal((*a).clone()));
                segs.extend(b.segments.iter().cloned());
                Self::from(Value::resolvable_string_value(
                    crate::value::ResolvableString::from_segments(segs),
                ))
            }
            (Value::resolvable_string_value(a), Value::str_value(b)) => {
                let mut segs = Vec::with_capacity(a.segments.len() + 1);
                segs.extend(a.segments.iter().cloned());
                segs.push(crate::value::Segment::Literal((*b).clone()));
                Self::from(Value::resolvable_string_value(
                    crate::value::ResolvableString::from_segments(segs),
                ))
            }
            (Value::resolvable_string_value(a), Value::resolvable_string_value(b)) => {
                let mut segs = Vec::with_capacity(a.segments.len() + b.segments.len());
                segs.extend(a.segments.iter().cloned());
                segs.extend(b.segments.iter().cloned());
                Self::from(Value::resolvable_string_value(
                    crate::value::ResolvableString::from_segments(segs),
                ))
            }
            // Mokkan F1.5: inet + int / int + inet (offset arithmetic).
            // Preserves source masklen; overflow panics per
            // inet_add_offset. F2.2: symbolic operand → AddOffset
            // wrapping (Resolved gets lifted to LiteralInet leaf).
            (Value::inet_value(a), Value::int_value(b)) => match &a.inner {
                crate::value::InetInner::Resolved(r) => Self::from(Value::inet_value(
                    crate::value::InetValue::resolved(crate::value::inet_add_offset(*r, *b)),
                )),
                crate::value::InetInner::Symbolic(expr) => {
                    let wrapped = crate::value::Expr::AddOffset(
                        Box::new(expr.clone()),
                        Box::new(crate::value::Expr::LiteralInt(*b)),
                    );
                    Self::from(Value::inet_value(crate::value::InetValue::symbolic(
                        wrapped,
                    )))
                }
            },
            (Value::int_value(a), Value::inet_value(b)) => match &b.inner {
                crate::value::InetInner::Resolved(r) => Self::from(Value::inet_value(
                    crate::value::InetValue::resolved(crate::value::inet_add_offset(*r, *a)),
                )),
                crate::value::InetInner::Symbolic(expr) => {
                    let wrapped = crate::value::Expr::AddOffset(
                        Box::new(expr.clone()),
                        Box::new(crate::value::Expr::LiteralInt(*a)),
                    );
                    Self::from(Value::inet_value(crate::value::InetValue::symbolic(
                        wrapped,
                    )))
                }
            },
            (Value::list_value(a), _) => {
                if x.is_list() {
                    let mut list = a.clone();
                    let b = x.as_list_ref();
                    for x in b.values.iter() {
                        list.values.push(x.clone());
                    }
                    Self::from(Value::list_value(list))
                } else {
                    let msg = format!(
                        "can only concatenate list (not \"{}\") to list",
                        x.type_str()
                    );
                    panic!("{}", msg);
                }
            }
            _ => panic_unsupported_bin_op!("+", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_sub(&self, ctx: &mut Context, x: &Self) -> Self {
        let strict_range_check_32 = ctx.cfg.strict_range_check;
        let strict_range_check_64 = ctx.cfg.debug_mode || !ctx.cfg.strict_range_check;

        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_i32_overflow_sub(*a, *b) {
                    panic_i32_overflow!(ctx, *a as i128 - *b as i128);
                }
                if strict_range_check_64 && is_i64_overflow_sub(*a, *b) {
                    panic_i32_overflow!(ctx, *a as i128 - *b as i128);
                }
                Self::int(*a - *b)
            }
            (Value::float_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_sub(*a, *b) {
                    panic_f32_overflow!(ctx, *a - *b);
                }
                Self::float(*a - *b)
            }
            (Value::int_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_sub(*a as f64, *b) {
                    panic_f32_overflow!(ctx, *a as f64 - *b);
                }
                Self::float(*a as f64 - *b)
            }
            (Value::float_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_sub(*a, *b as f64) {
                    panic_f32_overflow!(ctx, *a - *b as f64);
                }
                Self::float(*a - *b as f64)
            }
            // Mokkan F1.5: inet - int (offset arithmetic, preserves
            // masklen); inet - inet (signed distance, panics on v6
            // overflow). Same-family only — cross-family panics.
            // F2.2: symbolic `inet - int` lifts to SubOffset; symbolic
            // `inet - inet` is D4 err (the resulting int isn't usable
            // by any symbolic-int consumer at F2.2 — F2.4 may revisit).
            (Value::inet_value(a), Value::int_value(b)) => match &a.inner {
                crate::value::InetInner::Resolved(r) => {
                    let neg = b.checked_neg().unwrap_or_else(|| {
                        panic!("inet - int: cannot negate {b} (i64::MIN overflow)")
                    });
                    Self::from(Value::inet_value(crate::value::InetValue::resolved(
                        crate::value::inet_add_offset(*r, neg),
                    )))
                }
                crate::value::InetInner::Symbolic(expr) => {
                    let wrapped = crate::value::Expr::SubOffset(
                        Box::new(expr.clone()),
                        Box::new(crate::value::Expr::LiteralInt(*b)),
                    );
                    Self::from(Value::inet_value(crate::value::InetValue::symbolic(
                        wrapped,
                    )))
                }
            },
            (Value::inet_value(a), Value::inet_value(b)) => match (&a.inner, &b.inner) {
                (crate::value::InetInner::Resolved(ra), crate::value::InetInner::Resolved(rb)) => {
                    Self::int(crate::value::inet_distance(*ra, *rb))
                }
                _ => panic!(
                    "inet - inet on symbolic operands is D4 err — the resulting symbolic int \
                         has no value-level carrier at F2.2 (F2.4 may revisit via \
                         ResolvableString). Restructure to compute the distance against \
                         resolved values, or compose offsets before deriving."
                ),
            },
            _ => panic_unsupported_bin_op!("-", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_mul(&self, ctx: &mut Context, x: &Self) -> Self {
        let strict_range_check_32 = ctx.cfg.strict_range_check;
        let strict_range_check_64 = ctx.cfg.debug_mode || !ctx.cfg.strict_range_check;

        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_i32_overflow_mul(*a, *b) {
                    panic_i32_overflow!(ctx, *a as i128 * *b as i128);
                }
                if strict_range_check_64 && is_i64_overflow_mul(*a, *b) {
                    panic_i64_overflow!(ctx, *a as i128 * *b as i128);
                }
                Self::int(*a * *b)
            }
            (Value::float_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_mul(*a, *b) {
                    panic_f32_overflow!(ctx, *a * *b);
                }
                Self::float(*a * *b)
            }
            (Value::int_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_mul(*a as f64, *b) {
                    panic_f32_overflow!(ctx, *a as f64 * *b);
                }
                Self::float(*a as f64 * *b)
            }
            (Value::float_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_mul(*a, *b as f64) {
                    panic_f32_overflow!(ctx, *a * *b as f64);
                }
                Self::float(*a * *b as f64)
            }

            (Value::str_value(a), Value::int_value(b)) => Self::str(a.repeat(*b as usize).as_ref()),
            (Value::int_value(b), Value::str_value(a)) => Self::str(a.repeat(*b as usize).as_ref()),
            (Value::list_value(a), Value::int_value(b)) => {
                let mut list = ListValue::default();
                for _ in 0..(*b as usize) {
                    for x in a.values.iter() {
                        list.values.push(x.deep_copy());
                    }
                }
                Self::from(Value::list_value(Box::new(list)))
            }
            (Value::int_value(b), Value::list_value(a)) => {
                let mut list = ListValue::default();
                for _ in 0..(*b as usize) {
                    for x in a.values.iter() {
                        list.values.push(x.deep_copy());
                    }
                }
                Self::from(Value::list_value(Box::new(list)))
            }
            _ => panic_unsupported_bin_op!("*", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_div(&self, x: &Self) -> Self {
        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => Self::float((*a as f64) / (*b as f64)),
            (Value::float_value(a), Value::float_value(b)) => Self::float(*a / *b),
            (Value::int_value(a), Value::float_value(b)) => Self::float(*a as f64 / *b),
            (Value::float_value(a), Value::int_value(b)) => Self::float(*a / *b as f64),
            _ => panic_unsupported_bin_op!("/", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_mod(&self, x: &Self) -> Self {
        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                let x = *a;
                let y = *b;
                if (x < 0) != (y < 0) && x % y != 0 {
                    Self::int(x % y + y)
                } else {
                    Self::int(x % y)
                }
            }
            (Value::float_value(a), Value::float_value(b)) => Self::float(*a % *b),
            (Value::int_value(a), Value::float_value(b)) => Self::float(*a as f64 % *b),
            (Value::float_value(a), Value::int_value(b)) => Self::float(*a % *b as f64),
            _ => panic_unsupported_bin_op!("%", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_pow(&self, ctx: &mut Context, x: &Self) -> Self {
        let strict_range_check_32 = ctx.cfg.strict_range_check;
        let strict_range_check_64 = ctx.cfg.debug_mode || !ctx.cfg.strict_range_check;

        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_i32_overflow_pow(*a, *b) {
                    panic_i32_overflow!(ctx, (*a as i128).pow(*b as u32));
                }
                if strict_range_check_64 && is_i64_overflow_pow(*a, *b) {
                    panic_i64_overflow!(ctx, (*a as i128).pow(*b as u32));
                }
                Self::int(a.pow(*b as u32))
            }
            (Value::float_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_pow(*a, *b) {
                    panic_f32_overflow!(ctx, a.powf(*b));
                }
                Self::float(a.powf(*b))
            }
            (Value::int_value(a), Value::float_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_pow(*a as f64, *b) {
                    panic_f32_overflow!(ctx, (*a as f64).powf(*b));
                }
                Self::float((*a as f64).powf(*b))
            }
            (Value::float_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_f32_overflow_pow(*a, *b as f64) {
                    panic_f32_overflow!(ctx, a.powf(*b as f64));
                }
                Self::float(a.powf(*b as f64))
            }
            _ => panic_unsupported_bin_op!("**", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_floor_div(&self, x: &Self) -> Self {
        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                let x = *a;
                let y = *b;
                if (x < 0) != (y < 0) && x % y != 0 {
                    Self::int(x / y - 1)
                } else {
                    Self::int(x / y)
                }
            }
            (Value::float_value(a), Value::float_value(b)) => Self::float((*a / *b).floor()),
            (Value::int_value(a), Value::float_value(b)) => Self::float((*a as f64 / *b).floor()),
            (Value::float_value(a), Value::int_value(b)) => Self::float((*a / *b as f64).floor()),
            _ => panic_unsupported_bin_op!("//", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_bit_lshift(&self, ctx: &mut Context, x: &Self) -> Self {
        let strict_range_check_32 = ctx.cfg.strict_range_check;
        let strict_range_check_64 = ctx.cfg.debug_mode || !ctx.cfg.strict_range_check;

        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_i32_overflow_shl(*a, *b) {
                    panic_i32_overflow!(ctx, (*a as i128) << (*b as u32));
                }
                if strict_range_check_64 && is_i64_overflow_shl(*a, *b) {
                    panic_i64_overflow!(ctx, (*a as i128) << (*b as u32));
                }
                Self::int(*a << *b)
            }
            _ => panic_unsupported_bin_op!("<<", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_bit_rshift(&self, ctx: &mut Context, x: &Self) -> Self {
        let strict_range_check_32 = ctx.cfg.strict_range_check;
        let strict_range_check_64 = ctx.cfg.debug_mode || !ctx.cfg.strict_range_check;

        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => {
                if strict_range_check_32 && is_i32_overflow_shr(*a, *b) {
                    panic_i32_overflow!(ctx, (*a as i128) >> (*b as u32));
                }
                if strict_range_check_64 && is_i64_overflow_shr(*a, *b) {
                    panic_i64_overflow!(ctx, (*a as i128) >> (*b as u32));
                }
                Self::int(*a >> *b)
            }
            _ => panic_unsupported_bin_op!(">>", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_bit_and(&self, x: &Self) -> Self {
        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => Self::int(*a & *b),
            _ => panic_unsupported_bin_op!("&", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_bit_xor(&self, x: &Self) -> Self {
        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::int_value(a), Value::int_value(b)) => Self::int(*a ^ *b),
            _ => panic_unsupported_bin_op!("^", self.type_str(), x.type_str()),
        }
    }

    pub fn bin_bit_or(&self, ctx: &mut Context, x: &Self) -> Self {
        if let (Value::int_value(a), Value::int_value(b)) = (&*self.rc.borrow(), &*x.rc.borrow()) {
            return Self::int(*a | *b);
        };
        self.deep_copy()
            .union_entry(ctx, x, true, &UnionOptions::default())
    }

    pub fn bin_subscr(&self, x: &Self) -> Self {
        match (&*self.rc.borrow(), &*x.rc.borrow()) {
            (Value::str_value(a), Value::int_value(b)) => {
                let str_len = a.chars().count();
                let index = *b;
                let index = must_normalize_index(index as i32, str_len);
                if index < a.len() {
                    let ch = a.chars().nth(index).unwrap();
                    Self::str(ch.to_string().as_ref())
                } else {
                    panic!("string index out of range: {b}");
                }
            }
            (Value::list_value(a), Value::int_value(b)) => {
                let index = *b;
                let index = must_normalize_index(index as i32, a.values.len());
                if index < a.values.len() {
                    a.values[index].clone()
                } else {
                    panic!("list index out of range: {b}");
                }
            }
            (Value::dict_value(a), Value::str_value(b)) => match a.values.get(b) {
                Some(x) => (*x).clone(),
                _ => Self::undefined(),
            },
            (Value::dict_value(_), _) => Self::undefined(),
            (Value::schema_value(a), Value::str_value(b)) => match a.config.values.get(b) {
                Some(x) => (*x).clone(),
                _ => Self::undefined(),
            },
            _ => panic!(
                "'{}' object is not subscriptable with '{}'",
                self.type_str(),
                x.type_str()
            ),
        }
    }

    pub fn bin_subscr_option(&self, x: &Self) -> Self {
        if self.is_truthy() {
            self.bin_subscr(x)
        } else {
            Self::none()
        }
    }

    pub fn bin_subscr_set(&mut self, ctx: &mut Context, x: &Self, v: &Self) {
        if self.is_list() && x.is_int() {
            self.list_set_value(x, v);
        } else if self.is_config() && x.is_str() {
            let key = x.as_str();
            self.dict_set_value(ctx, &key, v);
        } else {
            panic!(
                "'{}' object is not subscriptable with '{}'",
                self.type_str(),
                x.type_str()
            );
        }
    }
}

#[cfg(test)]
mod test_value_bin {

    use crate::*;

    #[test]
    fn test_int_bin() {
        let mut ctx = Context::new();
        let cases = [
            (0, 0, "+", 0),
            (1, 1, "-", 0),
            (-1, 2, "*", -2),
            (4, 2, "/", 2),
            (-2, 4, "//", -1),
            (-2, 5, "%", 3),
            (3, 2, "**", 9),
            (2, 1, ">>", 1),
            (3, 2, "<<", 12),
            (5, 9, "&", 1),
            (5, 10, "|", 15),
            (7, 11, "^", 12),
        ];
        for (left, right, op, expected) in cases {
            let left = ValueRef::int(left);
            let right = ValueRef::int(right);
            let result = match op {
                "+" => left.bin_add(&mut ctx, &right),
                "-" => left.bin_sub(&mut ctx, &right),
                "*" => left.bin_mul(&mut ctx, &right),
                "/" => left.bin_div(&right),
                "//" => left.bin_floor_div(&right),
                "%" => left.bin_mod(&right),
                "**" => left.bin_pow(&mut ctx, &right),
                "<<" => left.bin_bit_lshift(&mut ctx, &right),
                ">>" => left.bin_bit_rshift(&mut ctx, &right),
                "&" => left.bin_bit_and(&right),
                "|" => left.bin_bit_or(&mut ctx, &right),
                "^" => left.bin_bit_xor(&right),
                _ => panic!("invalid op {}", op),
            };
            assert_eq!(result.as_int(), expected as i64)
        }
    }

    #[test]
    fn test_str_subscr() {
        let data = ValueRef::str("Hello world");
        let cases = [(0, "H"), (1, "e"), (-1, "d"), (-2, "l")];
        for (index, expected) in cases {
            let index = ValueRef::int(index as i64);
            let result = data.bin_subscr(&index).as_str();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_list_subscr() {
        let data = ValueRef::list_int(&[1, 2, 3, 4]);
        let cases = [(0, 1), (1, 2), (-1, 4), (-2, 3)];
        for (index, expected) in cases {
            let index = ValueRef::int(index as i64);
            let result = data.bin_subscr(&index).as_int();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_dict_subscr() {
        let data = ValueRef::dict_int(&[("k1", 1), ("k2", 2)]);
        assert_eq!(
            data.bin_subscr(&ValueRef::str("err_key")),
            ValueRef::undefined()
        );
        let cases = [("k1", 1), ("k2", 2)];
        for (key, expected) in cases {
            let key = ValueRef::str(key);
            let result = data.bin_subscr(&key).as_int();
            assert_eq!(result, expected);
        }
    }

    // ────────────────────────────────────────────────────────────────
    // F2.2: symbolic-arm dispatch on inet operator overloads.
    // ────────────────────────────────────────────────────────────────

    use crate::value::{Expr, InetInner, InetValue, IpFamily};

    const HANDLE: &str = "lan";

    /// Build a symbolic InetValue carrying a single HandleSubnet leaf.
    /// The handle-as-leaf is the canonical symbolic-inet shape that
    /// F2.5's `symbolic_inet` builtin will produce; this stand-in lets
    /// the operator-overload arms be tested before that wiring lands.
    fn sym_inet() -> ValueRef {
        let e = Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        };
        ValueRef::from(Value::inet_value(InetValue::symbolic(e)))
    }

    fn assert_inet_symbolic_matches(v: &ValueRef, want: Expr) {
        let borrow = v.rc.borrow();
        match &*borrow {
            Value::inet_value(iv) => match &iv.inner {
                InetInner::Symbolic(e) => assert_eq!(*e, want, "symbolic Expr mismatch"),
                InetInner::Resolved(_) => panic!("expected Symbolic, got Resolved"),
            },
            _ => panic!("expected inet_value variant"),
        }
    }

    /// F2.2 / D4: `symbolic_inet + int` → `Symbolic(AddOffset(handle, int))`.
    /// Source masklen / handle are preserved verbatim through the
    /// wrapping (no eager evaluation of the symbolic leaf).
    #[test]
    fn symbolic_inet_plus_int_wraps_in_add_offset() {
        let mut ctx = Context::new();
        let sym = sym_inet();
        let result = sym.bin_add(&mut ctx, &ValueRef::int(10));
        assert_inet_symbolic_matches(
            &result,
            Expr::AddOffset(
                Box::new(Expr::HandleSubnet {
                    subnet_handle: HANDLE.to_string(),
                }),
                Box::new(Expr::LiteralInt(10)),
            ),
        );
    }

    /// Commutative: `int + symbolic_inet` lands at the same Expr.
    #[test]
    fn int_plus_symbolic_inet_wraps_in_add_offset() {
        let mut ctx = Context::new();
        let sym = sym_inet();
        let result = ValueRef::int(10).bin_add(&mut ctx, &sym);
        assert_inet_symbolic_matches(
            &result,
            Expr::AddOffset(
                Box::new(Expr::HandleSubnet {
                    subnet_handle: HANDLE.to_string(),
                }),
                Box::new(Expr::LiteralInt(10)),
            ),
        );
    }

    /// F2.2: `symbolic_inet - int` → `Symbolic(SubOffset(handle, int))`.
    /// Note: this is `SubOffset`, not `AddOffset(handle, -int)` — the
    /// IR preserves the operator the source used, which the resolver
    /// can later inspect for diagnostics.
    #[test]
    fn symbolic_inet_minus_int_wraps_in_sub_offset() {
        let mut ctx = Context::new();
        let sym = sym_inet();
        let result = sym.bin_sub(&mut ctx, &ValueRef::int(5));
        assert_inet_symbolic_matches(
            &result,
            Expr::SubOffset(
                Box::new(Expr::HandleSubnet {
                    subnet_handle: HANDLE.to_string(),
                }),
                Box::new(Expr::LiteralInt(5)),
            ),
        );
    }

    /// F2.2 / D4: `symbolic_inet - symbolic_inet` is err. The
    /// resulting symbolic int has no value-level carrier until F2.4's
    /// ResolvableString lands; rather than half-implement, we panic
    /// with a deferral-pointing message.
    #[test]
    #[should_panic(expected = "inet - inet on symbolic operands is D4 err")]
    fn symbolic_inet_minus_symbolic_inet_panics() {
        let mut ctx = Context::new();
        let a = sym_inet();
        let b = sym_inet();
        let _ = a.bin_sub(&mut ctx, &b);
    }

    // ────────────────────────────────────────────────────────────────
    // F2.4: string concat with ResolvableString.
    // ────────────────────────────────────────────────────────────────

    use crate::value::{ResolvableString, Segment};

    const LITERAL_PREFIX: &str = "host-";
    const LITERAL_SUFFIX: &str = "-fin";

    /// Build a single-symbolic-segment ResolvableString as the stand-in
    /// for the F2.4 stringification output (`text(symbolic_inet)`).
    fn sym_rs() -> ValueRef {
        let rs = ResolvableString::from_symbolic(Expr::Text(Box::new(Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        })));
        ValueRef::from(Value::resolvable_string_value(rs))
    }

    fn assert_segments(v: &ValueRef, want: Vec<Segment>) {
        let borrow = v.rc.borrow();
        match &*borrow {
            Value::resolvable_string_value(rs) => {
                assert_eq!(rs.segments, want, "segment list mismatch");
            }
            _ => panic!("expected resolvable_string_value variant"),
        }
    }

    /// F2.4: str + ResolvableString → ResolvableString with the str
    /// as a Literal prefix segment followed by the RS's segments.
    #[test]
    fn str_plus_resolvable_string_prepends_literal() {
        let mut ctx = Context::new();
        let rs = sym_rs();
        let result = ValueRef::str(LITERAL_PREFIX).bin_add(&mut ctx, &rs);
        let inner_expr = Expr::Text(Box::new(Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        }));
        assert_segments(
            &result,
            vec![
                Segment::Literal(LITERAL_PREFIX.to_string()),
                Segment::Symbolic(Box::new(inner_expr)),
            ],
        );
    }

    /// Mirror: ResolvableString + str → segments + Literal suffix.
    #[test]
    fn resolvable_string_plus_str_appends_literal() {
        let mut ctx = Context::new();
        let rs = sym_rs();
        let result = rs.bin_add(&mut ctx, &ValueRef::str(LITERAL_SUFFIX));
        let inner_expr = Expr::Text(Box::new(Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        }));
        assert_segments(
            &result,
            vec![
                Segment::Symbolic(Box::new(inner_expr)),
                Segment::Literal(LITERAL_SUFFIX.to_string()),
            ],
        );
    }

    /// RS + RS → spliced segment list. Useful for composing multiple
    /// symbolic fragments (e.g., from separate `text()` calls).
    #[test]
    fn resolvable_string_plus_resolvable_string_splices_segments() {
        let mut ctx = Context::new();
        let a = sym_rs();
        let b = sym_rs();
        let result = a.bin_add(&mut ctx, &b);
        // Result should be 2 Symbolic segments — same shape repeated.
        let borrow = result.rc.borrow();
        match &*borrow {
            Value::resolvable_string_value(rs) => {
                assert_eq!(rs.segments.len(), 2, "expected 2 spliced segments");
                assert!(
                    matches!(&rs.segments[0], Segment::Symbolic(_)),
                    "first segment should be Symbolic"
                );
                assert!(
                    matches!(&rs.segments[1], Segment::Symbolic(_)),
                    "second segment should be Symbolic"
                );
            }
            _ => panic!("expected resolvable_string_value"),
        }
    }

    /// Three-part concat: `LITERAL_PREFIX + symbolic + LITERAL_SUFFIX`
    /// — the canonical multi-segment shape. Locks down left-
    /// associativity producing `[Literal, Symbolic, Literal]`.
    #[test]
    fn three_part_concat_produces_literal_symbolic_literal() {
        let mut ctx = Context::new();
        let intermediate = ValueRef::str(LITERAL_PREFIX).bin_add(&mut ctx, &sym_rs());
        let result = intermediate.bin_add(&mut ctx, &ValueRef::str(LITERAL_SUFFIX));
        let inner_expr = Expr::Text(Box::new(Expr::HandleSubnet {
            subnet_handle: HANDLE.to_string(),
        }));
        assert_segments(
            &result,
            vec![
                Segment::Literal(LITERAL_PREFIX.to_string()),
                Segment::Symbolic(Box::new(inner_expr)),
                Segment::Literal(LITERAL_SUFFIX.to_string()),
            ],
        );
    }
}
