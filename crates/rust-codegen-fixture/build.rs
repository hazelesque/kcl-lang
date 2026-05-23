//! Build script: generate Rust types from `schemas.k` at build time.
//!
//! Mirrors the workflow a real consumer (Tilley) will use — invoke
//! `kcl_rust_codegen::generate_to_string` on the fixture KCL source,
//! write the result to `$OUT_DIR/generated_types.rs`, and let the
//! including `src/lib.rs` pick it up via `include!`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let schema_path = manifest_dir.join("schemas.k");
    let kcl_source = fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", schema_path.display()));

    let rust_source = match kcl_rust_codegen::generate_to_string(&kcl_source) {
        Ok(s) => s,
        Err(e) => panic!("kcl-rust-codegen failed: {e}"),
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_path = out_dir.join("generated_types.rs");
    fs::write(&out_path, rust_source)
        .unwrap_or_else(|e| panic!("write {}: {e}", out_path.display()));

    println!("cargo:rerun-if-changed={}", schema_path.display());
    println!("cargo:rerun-if-changed=build.rs");
}
