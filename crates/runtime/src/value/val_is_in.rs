//! Copyright The KCL Authors. All rights reserved.

use crate::*;

// is

impl ValueRef {
    #[inline]
    pub fn is_undefined(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::undefined)
    }

    #[inline]
    pub fn is_none(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::none)
    }

    #[inline]
    pub fn is_bool(&self) -> bool {
        self.kind() == Kind::Bool
    }

    #[inline]
    pub fn is_int(&self) -> bool {
        self.kind() == Kind::Int
    }

    #[inline]
    pub fn is_float(&self) -> bool {
        self.kind() == Kind::Float
    }

    #[inline]
    pub fn is_str(&self) -> bool {
        self.kind() == Kind::Str
    }

    #[inline]
    pub fn is_list(&self) -> bool {
        self.kind() == Kind::List
    }

    #[inline]
    pub fn is_dict(&self) -> bool {
        self.kind() == Kind::Dict
    }

    #[inline]
    pub fn is_schema(&self) -> bool {
        self.kind() == Kind::Schema
    }

    // F1.6: mokkan native variant predicates. `kind()` lumps
    // cidr/inet under `Kind::Inet` and macaddr/macaddr8 under
    // `Kind::MacAddr` for C-ABI dispatch; the codegen-emitted
    // `from_cidr` / `from_inet` / `from_macaddr` / `from_macaddr8`
    // helpers need a finer distinction, so these matches go through
    // the Value variant directly.

    #[inline]
    pub fn is_cidr(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::cidr_value(_))
    }

    #[inline]
    pub fn is_inet(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::inet_value(_))
    }

    #[inline]
    pub fn is_macaddr(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::macaddr_value(_))
    }

    #[inline]
    pub fn is_macaddr8(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::macaddr8_value(_))
    }

    #[inline]
    pub fn is_ip_family(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::ip_family_value(_))
    }

    /// Phase C.1: predicate for `uuid_value`. Mirrors the
    /// macaddr/ip_family shape — no symbolic state, no extra check.
    #[inline]
    pub fn is_uuid(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::uuid_value(_))
    }

    /// F2.3: predicate for the deferred-string carrier. Distinct
    /// from `is_str` — a `ResolvableString` is structurally a list
    /// of segments, not a single resolved string. Schema fields
    /// declared as `str` reject this value (per the F2.6
    /// `T | ResolvableString` discipline).
    #[inline]
    pub fn is_resolvable_string(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::resolvable_string_value(_))
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        matches!(
            &*self.rc.borrow(),
            Value::int_value(_) | Value::float_value(_)
        )
    }

    #[inline]
    pub fn is_builtin(&self) -> bool {
        matches!(
            &*self.rc.borrow(),
            Value::int_value(_)
                | Value::float_value(_)
                | Value::bool_value(_)
                | Value::str_value(_)
                // F1.3: mokkan native types are primitive-shaped
                // (carry a single typed payload, not a collection or
                // schema). `match_builtin_type` walks `value.is_builtin()
                // && value.type_str() == tpe` for the check, so this
                // is what lets `field: cidr = "10.0.0.0/24"` accept
                // the coerced cidr_value at schema-validation time.
                | Value::cidr_value(_)
                | Value::inet_value(_)
                | Value::macaddr_value(_)
                | Value::macaddr8_value(_)
                | Value::ip_family_value(_)
                // Phase C.1: uuid is primitive-shaped — single
                // payload, no symbolic state. Schema fields declared
                // `field: uuid` accept the coerced uuid_value at
                // validation time via the same path.
                | Value::uuid_value(_)
                // F2.3: ResolvableString is primitive-shaped (single
                // payload, not a collection). is_builtin lets a field
                // declared `ResolvableString` accept the value at
                // schema-validation time via the same
                // `value.is_builtin() && value.type_str() == tpe`
                // path the other mokkan types use.
                | Value::resolvable_string_value(_)
        )
    }

    #[inline]
    pub fn is_config(&self) -> bool {
        matches!(
            &*self.rc.borrow(),
            Value::schema_value(_) | Value::dict_value(_)
        )
    }

    #[inline]
    pub fn is_list_or_config(&self) -> bool {
        matches!(
            &*self.rc.borrow(),
            Value::list_value(_) | Value::schema_value(_) | Value::dict_value(_)
        )
    }

    #[inline]
    pub fn is_func(&self) -> bool {
        self.kind() == Kind::Func
    }

    #[inline]
    pub fn is_none_or_undefined(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::none | Value::undefined)
    }

    #[inline]
    pub fn is_unit(&self) -> bool {
        matches!(&*self.rc.borrow(), Value::unit_value(..))
    }

    #[inline]
    pub fn is_scalar(&self) -> bool {
        matches!(
            &*self.rc.borrow(),
            Value::none
                | Value::bool_value(_)
                | Value::int_value(_)
                | Value::float_value(_)
                | Value::str_value(_)
                | Value::unit_value(..)
        )
    }
}

// in

impl ValueRef {
    pub fn r#in(&self, x: &Self) -> bool {
        match &*x.rc.borrow() {
            // "a" in "abc"
            Value::str_value(b) => match &*self.rc.borrow() {
                Value::str_value(a) => b.contains(a),
                _ => false,
            },
            // x in [1, 2, 3]
            Value::list_value(list) => {
                for v in list.values.as_slice().iter() {
                    if self.cmp_equal(v) {
                        return true;
                    }
                }
                false
            }
            // k in {k:v}
            Value::dict_value(dict) => {
                let key = self.as_str();
                dict.values.contains_key(&key)
            }
            // k in schema{}
            Value::schema_value(schema) => {
                let key = self.as_str();
                schema.config.values.contains_key(&key)
            }
            _ => {
                let msg = format!(
                    "TypeError: argument of type '{}' is not iterable",
                    x.type_str()
                );
                panic!("{}", msg);
            }
        }
    }

    pub fn not_in(&self, x: &Self) -> bool {
        !self.r#in(x)
    }
}

impl ValueRef {
    pub fn has_key(&self, key: &str) -> bool {
        match &*self.rc.borrow() {
            Value::dict_value(dict) => dict.values.contains_key(key),
            Value::schema_value(schema) => schema.config.values.contains_key(key),
            _ => false,
        }
    }

    pub fn has_value(&self, x: &Self) -> bool {
        x.r#in(self)
    }
}

#[cfg(test)]
mod test_value_in {
    use crate::*;

    #[test]
    fn test_in() {
        assert!(ValueRef::str("a").r#in(&ValueRef::str("abc")));
        assert!(ValueRef::str("ab").r#in(&ValueRef::str("abc")));
        assert!(!ValueRef::str("abcd").r#in(&ValueRef::str("abc")));

        assert!(ValueRef::str("a").r#in(&ValueRef::list_str(&[
            "a".to_string(),
            "b".to_string(),
            "c".to_string()
        ])));
        assert!(!ValueRef::str("d").r#in(&ValueRef::list_str(&[
            "a".to_string(),
            "b".to_string(),
            "c".to_string()
        ])));
        assert!(ValueRef::str("key1").r#in(&ValueRef::dict_str(&[
            ("key1", "value1"),
            ("key2", "value1"),
        ])));
        assert!(!ValueRef::str("err_key").r#in(&ValueRef::dict_str(&[
            ("key1", "value1"),
            ("key2", "value1"),
        ])));
    }
}
