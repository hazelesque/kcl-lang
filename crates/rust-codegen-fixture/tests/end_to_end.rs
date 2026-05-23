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
use kcl_runtime::ValueRef;

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

/// Runtime drift detection: the generated `TryFrom<&ValueRef>`
/// returns `Err` (rather than panicking or silently producing
/// garbage) when the input ValueRef lacks a required field. This is
/// the D4 seatbelt's load-bearing property — in a well-formed
/// deployment the VM would have rejected the input upstream, but if
/// codegen ever drifts from the schema the VM was given, this is the
/// safety net that surfaces the mismatch instead of UB.
///
/// Reviewer 1 + Reviewer 2 both flagged the absence of this test in
/// the Phase 4 MVP; landing it here closes the quality gap.
#[test]
fn missing_required_field_surfaces_as_try_from_err() {
    // Construct a Disk ValueRef directly via kcl-runtime, skipping
    // KCL evaluation (so we can build a ValueRef that the VM would
    // never produce — exactly the codegen-drift scenario). Omit the
    // required `size_gb` field.
    let label = ValueRef::str("data");
    let malformed = ValueRef::dict(Some(&[("label", &label)]));

    let result = Disk::try_from(&malformed);
    let err = result.expect_err("missing required field should surface as Err");
    assert!(
        err.contains("Disk.size_gb") && err.contains("missing"),
        "Err should name the missing field; got: {err:?}"
    );
}

/// Runtime drift detection (type mismatch case): a field's
/// runtime-presented type doesn't match the schema's declared type.
/// The seatbelt surfaces this as an `Err` whose message names the
/// expected and actual types.
#[test]
fn type_mismatch_on_field_surfaces_as_try_from_err() {
    // size_gb is `int` in the schema; provide a str instead.
    let bogus_size = ValueRef::str("not-an-int");
    let malformed = ValueRef::dict(Some(&[("size_gb", &bogus_size)]));

    let result = Disk::try_from(&malformed);
    let err = result.expect_err("type-mismatched field should surface as Err");
    assert!(
        err.contains("Disk.size_gb") && err.contains("expected int"),
        "Err should name the field and the expected type; got: {err:?}"
    );
}

/// Runtime drift detection (enum case): a string-literal-union value
/// not in the declared set surfaces as `Err`, not as a silent
/// default or a wrongly-mapped variant.
#[test]
fn unexpected_str_enum_literal_surfaces_as_try_from_err() {
    // VmState is "running" | "stopped" | "paused"; "exploded"
    // isn't in the set.
    let bogus_state = ValueRef::str("exploded");
    let result = VmState::try_from(&bogus_state);
    let err = result.expect_err("unexpected literal should surface as Err");
    assert!(
        err.contains("exploded")
            && (err.contains("running") || err.contains("expected one of")),
        "Err should mention both the unexpected literal and the allowed set; got: {err:?}"
    );
}

/// Build-time drift detection (documented; no automated test):
///
/// The generated code references each schema field by name in two
/// places: the `pub <field>: <type>` struct declaration and the
/// `_kcl_codegen_helpers::require(v, "<field>", ...)` call in the
/// `TryFrom` impl. A consumer crate that uses `vm.foo` will fail to
/// compile if `foo` is renamed in the source schema — Rust's type
/// system is the catcher, not a runtime check.
///
/// To verify this property manually:
/// 1. Rename a field in `schemas.k` (e.g. `name` → `hostname`).
/// 2. `cargo build -p kcl-rust-codegen-fixture` — succeeds (build.rs
///    regenerates the types).
/// 3. `cargo test -p kcl-rust-codegen-fixture` — fails to compile
///    because the tests still reference `vm.name`.
/// 4. The compile error names the field and the line, just like any
///    other Rust rename refactor.
///
/// (Encoded as a comment-doc rather than a `#[test]` because making
/// "the consumer fails to compile" automated requires either
/// `trybuild` scaffolding or a subprocess `cargo build` invocation —
/// both heavy. The property is structural: if the generated source
/// uses schema field names verbatim, rust catches the drift.)
#[test]
fn generated_source_uses_schema_field_names_verbatim() {
    // Codegen test the property at the generated-source level: if
    // the source `Vm.name` were renamed, the generated source would
    // no longer contain the literal "pub name:" — and downstream
    // code referencing `vm.name` would fail to compile.
    let src = include_str!("../schemas.k");
    let generated = kcl_rust_codegen::generate_to_string(src).expect("codegen");
    assert!(
        generated.contains("pub name: String,"),
        "generated source should declare Vm.name with the source field name verbatim"
    );
    assert!(
        generated.contains("_kcl_codegen_helpers::require(v, \"name\", \"Vm\""),
        "generated source should read Vm.name by the source field name verbatim"
    );
    assert!(
        generated.contains("pub size_gb: i64,"),
        "generated source should declare Disk.size_gb with the source field name verbatim"
    );
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
