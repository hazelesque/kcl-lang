//! Build script: generate Rust types from `schema.k` at build time
//! and write them to `$OUT_DIR/generated_types.rs`. The example's
//! `src/main.rs` includes that file verbatim.
//!
//! Mirrors the workflow a real consumer (Tilley) uses — read schema
//! source, hand it to `kcl_rust_codegen::generate_to_string`, write
//! the result. Errors panic with the codegen diagnostic in the
//! message so the cargo build output points straight at the problem.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let schema_path = manifest_dir.join("schema.k");
    let kcl_source = fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", schema_path.display()));

    let rust_source = match kcl_rust_codegen::generate_to_string(&kcl_source) {
        Ok(s) => s,
        Err(e) => panic!("kcl-rust-codegen failed: {e}"),
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let out_path = out_dir.join("generated_types.rs");
    fs::write(&out_path, rust_source)
        .unwrap_or_else(|e| panic!("write {}: {e}", out_path.display()));

    println!("cargo:rerun-if-changed={}", schema_path.display());
    println!("cargo:rerun-if-changed=build.rs");
}
