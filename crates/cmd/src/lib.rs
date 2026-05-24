//! The `libkcl` command-line interface.

#[macro_use]
extern crate clap;

pub mod run;
pub mod settings;
pub(crate) mod util;

#[cfg(test)]
mod tests;

use clap::{ArgAction, Command};

use std::io;

use anyhow::Result;
use run::run_command;

/// Run the KCL main command.
pub fn main(args: &[&str]) -> Result<()> {
    init_tracing_subscriber();
    let matches = app().arg_required_else_help(true).get_matches_from(args);
    // Sub commands
    match matches.subcommand() {
        Some(("run", sub_matches)) => run_command(sub_matches, &mut io::stdout()),
        Some(("version", _)) => {
            println!("{}", kcl_version::get_version_info());
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        Some(("server", _)) => kcl_api::service::jsonrpc::start_stdio_server(),
        _ => Ok(()),
    }
}

/// Install a tracing subscriber configured from the `KCL_LOG` env var.
///
/// `KCL_LOG=info` (the default when unset) shows the once-per-evaluation
/// spans from Phase 6b: `kcl_parse`, `kcl_resolve`, `kcl_evaluate`,
/// and the top-level `kcl_exec_program` / `kcl_exec_program_to_value` /
/// `kcl_embedded_evaluate` spans. `KCL_LOG=debug` adds hot-path debug
/// events; `KCL_LOG=trace` adds the most detailed events. Library
/// consumers (`kcl-embed`, `kcl-runner`, `kcl-api`) deliberately do
/// NOT install a subscriber — that's the binary's job. If no
/// subscriber is installed, all tracing macros are no-ops.
///
/// `try_init` is used so a second call (e.g. via a host that already
/// initialised tracing) silently no-ops rather than panicking. The
/// `let _ = ...` discards the resulting `Result`.
fn init_tracing_subscriber() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("KCL_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Get the CLI application including a run command and
/// a gPRC server command to interacting with external systems.
pub fn app() -> Command {
    Command::new("libkcl")
        .version(kcl_version::VERSION)
        .about("KCL main CLI.")
        .subcommand(
            Command::new("run")
            .about("run")
            .arg(arg!([input] ... "Specify the input files to run").num_args(0..))
            .arg(arg!(output: -o --output <output> "Specify the YAML output file path"))
            .arg(arg!(format: -f --format <format> "Specify the output format (yaml or json)"))
            .arg(arg!(setting: -Y --setting <setting> ... "Specify the input setting file").num_args(1..))
            .arg(arg!(verbose: -v --verbose "Print test information verbosely").action(ArgAction::Count))
            .arg(arg!(disable_none: -n --disable_none "Disable dumping None values"))
            .arg(arg!(strict_range_check: -r --strict_range_check "Do perform strict numeric range checks"))
            .arg(arg!(debug: -d --debug "Run in debug mode (for developers only)"))
            .arg(arg!(sort_keys: -k --sort_keys "Sort result keys"))
            .arg(arg!(show_hidden: -H --show_hidden "Display hidden attributes"))
            .arg(arg!(fast_eval: -K --fast_eval "Use the fast evaluation mode"))
            .arg(arg!(arguments: -D --argument <arguments> ... "Specify the top-level argument").num_args(1..))
            .arg(arg!(path_selector: -S --path_selector <path_selector> ... "Specify the path selector").num_args(1..))
            .arg(arg!(overrides: -O --overrides <overrides> ... "Specify the configuration override path and value").num_args(1..))
            .arg(arg!(target: --target <target> "Specify the target type"))
            .arg(arg!(recursive: -R --recursive "Compile the files directory recursively"))
            .arg(arg!(package_map: -E --external <package_map> ... "Mapping of package name and path where the package is located").num_args(1..)),
        )
    .subcommand(Command::new("server").about("Start a rpc server for APIs"))
    .subcommand(Command::new("version").about("Show the KCL version"))
}
