//! End-to-end runnable example: KCL schema → build.rs codegen →
//! embedded VM evaluation → typed Rust access.
//!
//! This is the artefact the plan calls for in its "End-to-end
//! verification path" section. It exercises:
//!
//! - **Phase 2** structured output: the VM returns a `ValueRef` tree,
//!   not a YAML/JSON string.
//! - **Phase 3** embedding API: schema is registered in-memory via
//!   `Embedded::register_module`, no tempdir or filesystem schema
//!   files needed at runtime.
//! - **Phase 4** codegen: `build.rs` invokes `kcl-rust-codegen` to
//!   produce Rust types with `TryFrom<&ValueRef>` impls, so the
//!   Rust compiler knows the field types end-to-end.
//! - **Phase 6b** logging: a `KCL_LOG=info` subscriber installed by
//!   the example will surface the boundary spans
//!   (`kcl_embedded_evaluate` / `kcl_parse` / `kcl_resolve` /
//!   `kcl_evaluate`). Run with `KCL_LOG=info cargo run -p ...` to
//!   see them; without `KCL_LOG` set, the spans are no-ops.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p embed-with-codegen-example
//! ```

use anyhow::{Context, Result};
use kcl_embed::{Embedded, EvaluateArgs};

// The schema source, compile-time-aligned with the generated types
// (build.rs reads the same file). include_str! means a stale .k file
// can't drift from the generated Rust — both come from one source.
const SCHEMA_SOURCE: &str = include_str!("../schema.k");

/// Generated `Vm`, `Disk`, and `VmState`/`DiskStorageClass` lifted
/// enums live in this module. The `#![allow(...)]` suppressions
/// inside the generated source need to be inner attrs on a module
/// rather than the crate root, so the `include!` is wrapped here.
mod generated {
    #![allow(non_snake_case, dead_code, clippy::derive_partial_eq_without_eq)]
    include!(concat!(env!("OUT_DIR"), "/generated_types.rs"));
}

use generated::{Disk, DiskStorageClass, Vm, VmState};

fn main() -> Result<()> {
    let mut embedded = Embedded::new();
    embedded
        .register_module("infra", SCHEMA_SOURCE)
        .context("registering infra schema module")?;
    let ready = embedded.build();

    // The "user program" — a small fragment of KCL that imports the
    // registered schema module and constructs instances. In a real
    // consumer (Tilley), this would be the user's `Tilleyfile.l`.
    // Here it's inline so the example stays self-contained.
    //
    // Includes a `print()` call so the example demonstrates the
    // `log_messages` channel — the library does not write to stderr
    // itself; callers decide what to do with print output.
    let user_program = r#"
import infra

print("evaluating user program")

vm = infra.Vm {
    name = "alpha"
    cores = 4
    notes = "production frontend"
    disks = [
        infra.Disk {size_gb = 100, storage_class = "nvme"}
        infra.Disk {size_gb = 500, label = "data"}
    ]
    state = "running"
}
"#;

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: user_program.to_string(),
            ..EvaluateArgs::default()
        })
        .map_err(|e| anyhow::anyhow!("KCL evaluation failed: {e:?}"))?;

    // log_messages carries anything the user program emitted via
    // print(). In production a consumer might forward these to its
    // own logger or display them in a UI. The library itself never
    // writes to stderr — that's the consumer's call.
    for msg in &outcome.log_messages {
        eprintln!("[kcl] {msg}");
    }

    // Walk to `vm` in the result dict. The top-level value of any
    // KCL evaluation is a dict whose keys are the program's top-level
    // variable names.
    let vm_value = outcome
        .value
        .dict_get_value("vm")
        .context("vm binding missing from evaluation result")?;

    // The load-bearing line — chain of trust completes here. VM has
    // already enforced the schema (check blocks, type checks); the
    // generated TryFrom verifies the structural shape and produces
    // a value Rust's type system can reason about. No YAML in the
    // middle, no hand-written struct duplicating the schema.
    // Generated TryFrom uses String as its error type (see the
    // rustdoc on the generated impl — "Err is returned only when the
    // input ValueRef does not match the expected schema shape" — so
    // String is sufficient). Convert via map_err to feed anyhow.
    let vm: Vm = (&vm_value)
        .try_into()
        .map_err(|e: String| anyhow::anyhow!("Vm conversion: {e}"))?;

    // Typed access from here on. Rust knows the field types.
    println!("vm.name = {:?}", vm.name);
    println!("vm.memory_mb = {} (defaulted)", vm.memory_mb);
    println!("vm.cores = {}", vm.cores);
    println!("vm.notes = {:?}", vm.notes);
    println!("vm.state = {:?}", vm.state);
    assert_eq!(vm.state, VmState::Running, "state field round-tripped");
    assert_eq!(vm.memory_mb, 1024, "default flowed through unchanged");
    assert!(vm.notes.is_some(), "notes provided in user program");

    println!("vm has {} disks:", vm.disks.len());
    for (i, disk) in vm.disks.iter().enumerate() {
        println!(
            "  disks[{i}] = Disk {{ size_gb: {}, label: {:?}, storage_class: {:?} }}",
            disk.size_gb, disk.label, disk.storage_class,
        );
    }

    // Specific check: disks[0]'s explicit storage_class lift to enum.
    assert_eq!(vm.disks[0].storage_class, DiskStorageClass::Nvme);
    // disks[1] uses the default; default flow through codegen.
    assert_eq!(vm.disks[1].storage_class, DiskStorageClass::Ssd);
    // disks[1].label was the only one to set the optional field.
    assert_eq!(vm.disks[0].label, None);
    assert_eq!(vm.disks[1].label.as_deref(), Some("data"));

    let total_disk_gb: i64 = vm.disks.iter().map(|d| d.size_gb).sum();
    println!("total disk capacity: {total_disk_gb} GB");

    // Function signature in Rust — Vm and Disk are real types here.
    fn assertion_about_typed_disks(disks: &[Disk]) -> bool {
        disks.iter().all(|d| d.size_gb > 0)
    }
    assert!(assertion_about_typed_disks(&vm.disks));

    println!("ok: end-to-end pipeline produced typed Rust access");
    Ok(())
}
