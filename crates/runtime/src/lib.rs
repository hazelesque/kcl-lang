//! Copyright The KCL Authors. All rights reserved.

// C ABI opaque-pointer types exposed to the cdylib (Go consumes these via cgo).
// The LLVM IR annotations that used to accompany these were removed when the
// LLVM codegen path was deleted upstream.

// api-spec:       kcl_context_t
// api-spec(c):    typedef struct kcl_context_t kcl_context_t;

// api-spec:       kcl_eval_scope_t
// api-spec(c):    typedef struct kcl_eval_scope_t kcl_eval_scope_t;

// api-spec:       kcl_type_t
// api-spec(c):    typedef struct kcl_type_t kcl_type_t;

// api-spec:       kcl_value_t
// api-spec(c):    typedef struct kcl_value_t kcl_value_t;

// api-spec:       kcl_value_ref_t
// api-spec(c):    typedef struct kcl_value_ref_t kcl_value_ref_t;

// api-spec:       kcl_iterator_t
// api-spec(c):    typedef struct kcl_iterator_t kcl_iterator_t;

// api-spec:       kcl_buffer_t
// api-spec(c):    typedef struct kcl_buffer_t kcl_buffer_t;

// api-spec:       kcl_kind_t
// api-spec(c):    typedef enum kcl_kind_t kcl_kind_t;

// api-spec:       kcl_size_t
// api-spec(c):    typedef int32_t kcl_size_t;

// api-spec:       kcl_char_t
// api-spec(c):    typedef char kcl_char_t;

// api-spec:       kcl_bool_t
// api-spec(c):    typedef int8_t kcl_bool_t;

// api-spec:       kcl_int_t
// api-spec(c):    typedef int64_t kcl_int_t;

// api-spec:       kcl_float_t
// api-spec(c):    typedef double kcl_float_t;

// api-spec:       kcl_decorator_value_t
// api-spec(c):    typedef struct kcl_decorator_value_t kcl_decorator_value_t;

pub mod api;
pub use self::api::*;

pub mod context;
pub use self::context::*;

pub mod types;
pub use self::types::*;

pub mod unification;

pub mod value;
pub use self::value::*;

pub mod base32;
pub use self::base32::*;

pub mod base64;
pub use self::base64::*;

pub mod collection;
pub use self::collection::*;

pub mod crypto;
pub use self::crypto::*;

mod eval;

pub mod datetime;
pub use self::datetime::*;

pub mod encoding;

pub mod json;
pub use self::json::*;

pub mod manifests;
pub use self::manifests::*;

pub mod math;
pub use self::math::*;

// F1.7: upstream KCL's stringly-typed `net` package was deleted —
// mokkan.net (typed inet algebra) supersedes it. The three
// non-CIDR survivors (`fqdn`, `split_host_port`, `join_host_port`)
// were rehomed to `crates/runtime/src/mokkan_net/mod.rs` as
// `kcl_mokkan_net_*` per F1.4.bis / D6.

pub mod regex;
pub use self::regex::*;

pub mod stdlib;
pub use self::stdlib::*;

pub mod units;
pub use self::units::*;

pub mod yaml;
pub use self::yaml::*;

pub mod file;
pub use self::file::*;

pub mod template;
pub use self::template::*;

pub mod panic;
pub use self::panic::*;

pub mod addr;
pub use self::addr::*;

// F1.4: mokkan native packages.
pub mod mokkan_net;
pub use self::mokkan_net::*;

// F2.5: mokkan symbolic primitives — `net_symbolic.symbolic_subnet` /
// `net_symbolic.symbolic_inet`. Separate package from `mokkan.net`
// because symbolic mode is a spec extension on top of the typed
// types, not a property of the types themselves (per D6).
pub mod mokkan_net_symbolic;
pub use self::mokkan_net_symbolic::*;

// Phase C.2: mokkan.uuid — `uuid.parse(s)` + `uuid.v5(ns, name)`.
// UUID constructors for cross-project namespace anchors. No `v4()`
// builtin (random at eval time would break determinism); operators
// pass random UUIDs in via external_args (Tilley's host_uuid path).
pub mod mokkan_uuid;
pub use self::mokkan_uuid::*;

#[derive(Debug, Default, Clone)]
pub struct RuntimePanicRecord {
    pub kcl_panic_info: bool,
    pub message: String,
    pub rust_file: String,
    pub rust_line: i32,
    pub rust_col: i32,
}
