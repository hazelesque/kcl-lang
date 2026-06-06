//! Copyright The KCL Authors. All rights reserved.

use crate::*;

// common
impl ValueRef {
    pub fn kind(&self) -> Kind {
        match &*self.rc.borrow() {
            Value::undefined => Kind::Undefined,
            Value::none => Kind::None,
            Value::bool_value(_) => Kind::Bool,
            Value::int_value(_) => Kind::Int,
            Value::float_value(_) => Kind::Float,
            Value::str_value(_) => Kind::Str,
            Value::list_value(_) => Kind::List,
            Value::dict_value(_) => Kind::Dict,
            Value::schema_value(_) => Kind::Schema,
            Value::func_value(_) => Kind::Func,
            Value::unit_value(..) => Kind::Unit,
            // F1.1: mokkan native types coalesce to family-level
            // Kind variants for C-ABI runtime dispatch. type_str()
            // still distinguishes at the variant level.
            Value::cidr_value(_) | Value::inet_value(_) => Kind::Inet,
            Value::macaddr_value(_) | Value::macaddr8_value(_) => Kind::MacAddr,
            Value::ip_family_value(_) => Kind::IpFamily,
            Value::resolvable_string_value(_) => Kind::ResolvableString,
        }
    }
}
