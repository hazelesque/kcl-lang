//! Dump the generated Rust source for a small fixture to stdout.
//! Useful for eyeballing what codegen produces while iterating on
//! the emitter.
//!
//! Run with `cargo run --example dump -p kcl-rust-codegen`.

fn main() {
    let src = concat!(
        "schema Disk:\n",
        "    size_gb: int\n",
        "\n",
        "schema Vm:\n",
        "    name: str\n",
        "    memory_mb: int = 1024\n",
        "    notes?: str\n",
        "    disks: [Disk]\n",
    );
    match kcl_rust_codegen::generate_to_string(src) {
        Ok(out) => print!("{out}"),
        Err(e) => eprintln!("codegen failed: {e}"),
    }
}
