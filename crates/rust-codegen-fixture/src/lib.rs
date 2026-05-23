//! Holds the kcl-rust-codegen-generated types for the fixture
//! `schemas.k`. The build script writes `$OUT_DIR/generated_types.rs`
//! which this file includes verbatim, wrapped in a module so any
//! lint suppressions on the generated content stay scoped.

#![allow(non_snake_case, dead_code, clippy::derive_partial_eq_without_eq)]

include!(concat!(env!("OUT_DIR"), "/generated_types.rs"));
