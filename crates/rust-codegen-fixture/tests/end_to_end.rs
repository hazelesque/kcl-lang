//! End-to-end test: codegen runs at build time against `schemas.k`,
//! the generated types are compiled into this crate, kcl-embed
//! evaluates a KCL program against the same schemas, and the result
//! `ValueRef` is `try_into()`d directly into the generated Rust
//! types.
//!
//! This is the Phase 4 step 6 proof that the foundation works: VM
//! validates → ValueRef structured-output → generated TryFrom →
//! typed Rust. No JSON. No hand-written struct definitions.

use kcl_embed::{Embedded, EvaluateArgs};
use kcl_rust_codegen_fixture::{Disk, DiskStorageClass, Vm, VmState};

const VM_PROGRAM: &str = include_str!("../schemas.k");

#[test]
fn generated_types_roundtrip_a_real_vm() {
    let mut embedded = Embedded::new();
    embedded
        .register_module("schemas", VM_PROGRAM)
        .expect("register schemas module");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import schemas\n",
                "vm = schemas.Vm {\n",
                "    name = \"alpha\"\n",
                "    memory_mb = 4096\n",
                "    notes = \"primary build host\"\n",
                "    disks = [\n",
                "        schemas.Disk {size_gb = 100, label = \"root\"}\n",
                "        schemas.Disk {size_gb = 500}\n",
                "    ]\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let vm_ref = outcome
        .value
        .dict_get_value("vm")
        .expect("vm should be at top level");
    let vm = Vm::try_from(&vm_ref).expect("vm should try_into cleanly");

    assert_eq!(vm.name, "alpha");
    assert_eq!(vm.memory_mb, 4096);
    assert_eq!(vm.cores, 2, "default should flow through");
    assert_eq!(vm.notes, Some("primary build host".to_string()));
    assert_eq!(vm.disks.len(), 2);
    assert_eq!(vm.disks[0].size_gb, 100);
    assert_eq!(vm.disks[0].label, Some("root".to_string()));
    assert_eq!(vm.disks[1].size_gb, 500);
    assert_eq!(
        vm.disks[1].label, None,
        "unset optional field should map to None per D5"
    );

    // String-literal-union enum: default flows through and matches
    // the source literal.
    assert_eq!(vm.state, VmState::Stopped);
    assert_eq!(vm.disks[0].storage_class, DiskStorageClass::Ssd);
    assert_eq!(vm.disks[1].storage_class, DiskStorageClass::Ssd);
}

/// String-literal-union enum codegen end-to-end: the user program
/// sets `state = "running"`; the generated `VmState` enum's TryFrom
/// successfully maps it to the corresponding variant.
#[test]
fn string_literal_union_codegens_to_enum_with_variants() {
    let mut embedded = Embedded::new();
    embedded
        .register_module("schemas", VM_PROGRAM)
        .expect("register");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import schemas\n",
                "vm = schemas.Vm {\n",
                "    name = \"alpha\"\n",
                "    state = \"running\"\n",
                "    disks = [schemas.Disk {size_gb = 100, storage_class = \"nvme\"}]\n",
                "}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let vm = Vm::try_from(&outcome.value.dict_get_value("vm").unwrap()).expect("try_into");
    assert_eq!(vm.state, VmState::Running);
    assert_eq!(vm.disks[0].storage_class, DiskStorageClass::Nvme);
}

/// Defaults flow through end-to-end: a Vm constructed with only the
/// required field uses defaults for everything else, and the
/// generated types receive the populated values from the VM.
#[test]
fn defaults_flow_through_codegen() {
    let mut embedded = Embedded::new();
    embedded
        .register_module("schemas", VM_PROGRAM)
        .expect("register");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import schemas\n",
                "vm = schemas.Vm {name = \"beta\", disks = []}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let vm = Vm::try_from(&outcome.value.dict_get_value("vm").unwrap())
        .expect("try_into");
    assert_eq!(vm.name, "beta");
    assert_eq!(vm.memory_mb, 1024, "default 1024 should flow through");
    assert_eq!(vm.cores, 2, "default 2 should flow through");
    assert_eq!(vm.notes, None);
    assert!(vm.disks.is_empty());
}

/// Sanity-check that the generated Disk type also stands on its own:
/// can be constructed from a bare Disk ValueRef without going through
/// a Vm wrapper.
#[test]
fn disk_type_standalone_roundtrip() {
    let mut embedded = Embedded::new();
    embedded
        .register_module("schemas", VM_PROGRAM)
        .expect("register");
    let ready = embedded.build();

    let outcome = ready
        .evaluate(EvaluateArgs {
            main_source: concat!(
                "import schemas\n",
                "d = schemas.Disk {size_gb = 250, label = \"data\"}\n",
            )
            .to_string(),
            ..EvaluateArgs::default()
        })
        .expect("evaluate");

    let disk = Disk::try_from(&outcome.value.dict_get_value("d").unwrap())
        .expect("try_into");
    assert_eq!(disk.size_gb, 250);
    assert_eq!(disk.label, Some("data".to_string()));
}
