//! Copyright The KCL Authors. All rights reserved.

use crate::*;

impl ValueRef {
    #[inline]
    pub fn is_truthy(&self) -> bool {
        match &*self.rc.borrow() {
            Value::undefined => false,
            Value::none => false,
            Value::bool_value(v) => *v,
            Value::int_value(v) => *v != 0,
            Value::float_value(v) => *v != 0.0,
            Value::str_value(v) => !v.is_empty(),
            Value::list_value(v) => !v.values.is_empty(),
            Value::dict_value(v) => !v.values.is_empty(),
            Value::schema_value(v) => !v.config.values.is_empty(),
            Value::func_value(_) => true,
            Value::unit_value(v, _, _) => *v != 0.0,
            // F1.1: mokkan native types are always truthy when
            // present. There's no meaningful "empty cidr" or "zero
            // inet" — a parsed value exists or it doesn't. (Bare
            // 0.0.0.0/0 or 0.0.0.0 are valid network/host values
            // and not "falsy".)
            Value::cidr_value(_) => true,
            Value::inet_value(_) => true,
            Value::macaddr_value(_) => true,
            Value::macaddr8_value(_) => true,
            Value::ip_family_value(_) => true,
            // Phase C.1: UUID always truthy — same shape as other
            // primitive payloads. The all-zero (nil) UUID is still
            // a value, not "empty".
            Value::uuid_value(_) => true,
            // F2.3: truthy by "is there any segment". Empty segment
            // list is falsy (matches str's "empty is falsy" rule);
            // any non-empty segment list — even purely-symbolic that
            // would resolve to non-empty — is truthy. The truthiness
            // check is consulted by KCL evaluation before resolution
            // so we can't peek inside symbolic segments.
            Value::resolvable_string_value(v) => !v.segments.is_empty(),
        }
    }

    #[inline]
    pub fn logic_and(&self, x: &ValueRef) -> bool {
        self.is_truthy() && x.is_truthy()
    }

    #[inline]
    pub fn logic_or(&self, x: &ValueRef) -> bool {
        self.is_truthy() || x.is_truthy()
    }
}

#[cfg(test)]
mod test_value_logic {
    use crate::*;

    #[test]
    fn test_is_truthy() {
        let cases = [
            // false cases
            (ValueRef::int(1), true),
            (ValueRef::float(2.0f64), true),
            (ValueRef::bool(true), true),
            (ValueRef::str("s"), true),
            (ValueRef::list_int(&[0]), true),
            (ValueRef::dict_str(&[("key", "value")]), true),
            // true cases
            (ValueRef::undefined(), false),
            (ValueRef::none(), false),
            (ValueRef::int(0), false),
            (ValueRef::float(0.0f64), false),
            (ValueRef::bool(false), false),
            (ValueRef::str(""), false),
            (ValueRef::list(Some(&[])), false),
            (ValueRef::dict(Some(&[])), false),
        ];
        for (value, expected) in cases {
            let result = value.is_truthy();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_logic_and() {
        let cases = [
            // true cases
            (ValueRef::int(1), ValueRef::int(1), true),
            // false cases
            (ValueRef::int(0), ValueRef::int(0), false),
            (ValueRef::int(0), ValueRef::int(1), false),
            (ValueRef::int(1), ValueRef::int(0), false),
        ];
        for (left, right, expected) in cases {
            let result = left.logic_and(&right);
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_logic_or() {
        let cases = [
            // true cases
            (ValueRef::int(1), ValueRef::int(1), true),
            (ValueRef::int(0), ValueRef::int(1), true),
            (ValueRef::int(1), ValueRef::int(0), true),
            // false cases
            (ValueRef::int(0), ValueRef::int(0), false),
        ];
        for (left, right, expected) in cases {
            let result = left.logic_or(&right);
            assert_eq!(result, expected);
        }
    }

    // F2.3: ResolvableString truthiness — empty segment list is falsy
    // (matches str's empty-is-falsy rule), any non-empty list is
    // truthy (we can't peek inside symbolic segments at runtime).
    #[test]
    fn test_resolvable_string_truthiness() {
        let empty = ValueRef::from(Value::resolvable_string_value(
            crate::value::ResolvableString::new(),
        ));
        assert!(!empty.is_truthy(), "empty ResolvableString is falsy");

        let lit = ValueRef::from(Value::resolvable_string_value(
            crate::value::ResolvableString::from_literal("hello"),
        ));
        assert!(lit.is_truthy(), "literal-only ResolvableString is truthy");

        let sym = ValueRef::from(Value::resolvable_string_value(
            crate::value::ResolvableString::from_symbolic(crate::value::Expr::HandleSubnet {
                handle: "lan".to_string(),
                size: Some(24),
                family: Some(crate::value::IpFamily::V4),
            }),
        ));
        assert!(
            sym.is_truthy(),
            "symbolic-segment ResolvableString is truthy"
        );
    }
}
