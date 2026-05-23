//! `embed_hello` — runnable demo of the Phase 3 embedding API.
//!
//! Registers a Tilley-shaped schema in-memory, evaluates a user
//! program that imports it, walks the structured result tree and
//! prints each field. No tempdir, no protobuf round-trip, no
//! JSON/YAML re-parse — straight from the VM's [`ValueRef`] to
//! typed Rust access.
//!
//! Run with `cargo run --example embed_hello -p kcl-embed`.

use std::process::ExitCode;

use kcl_embed::{Embedded, EvaluateArgs, EvaluationError};

fn main() -> ExitCode {
    let mut embedded = Embedded::new();

    // Register a Tilley-shaped VM schema. The host owns the source
    // text; the embedded VM will resolve `import tilley` against this
    // registration when the user's program below is evaluated.
    if let Err(err) = embedded.register_module(
        "tilley",
        concat!(
            "schema Vm:\n",
            "    name: str\n",
            "    memory_mb: int = 1024\n",
            "    cores: int = 2\n",
        ),
    ) {
        eprintln!("register_module failed: {err}");
        return ExitCode::FAILURE;
    }

    let ready = embedded.build();

    let outcome = ready.evaluate(EvaluateArgs {
        main_source: concat!(
            "import tilley\n",
            "vm = tilley.Vm {name = \"alpha\", memory_mb = 4096}\n",
        )
        .to_string(),
        ..EvaluateArgs::default()
    });

    match outcome {
        Ok(outcome) => {
            // Walk the structured ValueRef. In a real consumer this
            // is where a kcl-rust-codegen-emitted TryFrom impl would
            // produce a typed `Vm` struct directly; for the demo we
            // walk the tree by hand to make the structure visible.
            let vm = match outcome.value.dict_get_value("vm") {
                Some(v) => v,
                None => {
                    eprintln!("expected `vm` at top level; got: {:?}", outcome.value);
                    return ExitCode::FAILURE;
                }
            };
            println!("vm.name      = {}", vm.dict_get_value("name").unwrap().as_str());
            println!(
                "vm.memory_mb = {}",
                vm.dict_get_value("memory_mb").unwrap().as_int()
            );
            println!(
                "vm.cores     = {}  (default flowed through)",
                vm.dict_get_value("cores").unwrap().as_int()
            );
            if vm.is_schema() {
                println!("vm._schema   = {}", vm.as_schema().name);
            }
            for line in &outcome.log_messages {
                println!("[print()] {line}");
            }
            ExitCode::SUCCESS
        }
        Err(EvaluationError::Parse(diags)) => {
            eprintln!("parse error ({} diagnostics):", diags.len());
            for d in diags {
                for m in &d.messages {
                    eprintln!("  {}", m.message);
                }
            }
            ExitCode::FAILURE
        }
        Err(EvaluationError::Resolve(diags)) => {
            eprintln!("resolve error ({} diagnostics):", diags.len());
            for d in diags {
                for m in &d.messages {
                    eprintln!("  {}", m.message);
                }
            }
            ExitCode::FAILURE
        }
        Err(EvaluationError::Evaluate(diags)) => {
            eprintln!("evaluation error ({} diagnostics):", diags.len());
            for d in diags {
                for m in &d.messages {
                    eprintln!("  {}", m.message);
                }
            }
            ExitCode::FAILURE
        }
        Err(EvaluationError::Internal(err)) => {
            eprintln!("internal failure: {err}");
            ExitCode::FAILURE
        }
    }
}
