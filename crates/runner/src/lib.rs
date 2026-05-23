use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use anyhow::{Result, bail};
use kcl_ast::{
    MAIN_PKG,
    ast::{Module, Program},
};
use kcl_parser::{KCLModuleCache, ParseSessionRef, load_program};
use kcl_query::apply_overrides;
use kcl_sema::resolver::{
    Options, resolve_program, resolve_program_with_opts, scope::ProgramScope,
};
pub use runner::{ExecProgramArgs, ExecProgramResult, ExecProgramValueResult, MapErrorResult};
use runner::{FastRunner, RunnerOptions};

pub mod runner;

#[cfg(test)]
pub mod tests;

/// Load, resolve, and evaluate a KCL program from `args.k_filename_list`,
/// returning the JSON+YAML result.
///
/// Parses the files via `kcl_parser::load_program`, applies any overrides,
/// then delegates to [`execute`] for resolution + evaluation.
///
/// **Note that it is not thread safe.**
///
/// # Examples
///
/// ```
/// use kcl_runner::{exec_program, ExecProgramArgs};
/// use kcl_parser::ParseSession;
/// use std::sync::Arc;
///
/// let sess = Arc::new(ParseSession::default());
/// let mut args = ExecProgramArgs::default();
/// args.k_filename_list = vec!["./src/test_datas/init_check_order_0/main.k".to_string()];
///
/// let result = exec_program(sess, &args).unwrap();
/// ```
pub fn exec_program(sess: ParseSessionRef, args: &ExecProgramArgs) -> Result<ExecProgramResult> {
    // parse args from json string
    let opts = args.get_load_program_options();
    let kcl_paths_str = args
        .k_filename_list
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<&str>>();
    let module_cache = KCLModuleCache::default();
    let mut program = load_program(
        sess.clone(),
        kcl_paths_str.as_slice(),
        Some(opts),
        Some(module_cache),
    )?
    .program;
    apply_overrides(
        &mut program,
        &args.overrides,
        &[],
        args.print_override_ast || args.debug > 0,
    )?;
    execute(sess, program, args)
}

/// Resolve and evaluate an already-parsed KCL [`Program`], returning the
/// JSON+YAML result.
///
/// Runs sema resolution, emits any parse/resolve diagnostics, then hands
/// the resolved program to the tree-walking [`FastRunner`] for evaluation.
/// If `args.compile_only` is set, sema runs but evaluation is skipped.
///
/// **Note that it is not thread safe.**
///
/// # Examples
///
/// ```
/// use kcl_runner::{execute, runner::ExecProgramArgs};
/// use kcl_parser::{load_program, ParseSession};
/// use kcl_ast::ast::Program;
/// use std::sync::Arc;
///
/// let sess = Arc::new(ParseSession::default());
/// let args = ExecProgramArgs::default();
/// let opts = args.get_load_program_options();
///
/// let kcl_path = "./src/test_datas/init_check_order_0/main.k";
/// let prog = load_program(sess.clone(), &[kcl_path], Some(opts), None).unwrap().program;
///
/// let result = execute(sess, prog, &args).unwrap();
/// ```
pub fn execute(
    sess: ParseSessionRef,
    mut program: Program,
    args: &ExecProgramArgs,
) -> Result<ExecProgramResult> {
    // If the user only wants to compile the kcl program, the following code will only resolve ast.
    if args.compile_only {
        let resolve_opts = Options {
            merge_program: false,
            ..Default::default()
        };
        // Resolve ast
        let scope = resolve_program_with_opts(&mut program, resolve_opts, None);
        emit_compile_diag_to_string(sess, &scope, args.compile_only)?;
        return Ok(ExecProgramResult::default());
    }
    // Resolve ast
    let scope = resolve_program(&mut program);
    // Emit parse and resolve errors if exists.
    emit_compile_diag_to_string(sess, &scope, false)?;
    FastRunner::new(Some(RunnerOptions {
        plugin_agent_ptr: args.plugin_agent,
    }))
    .run(&program, args)
}

/// Load, resolve, and evaluate a KCL program from `args.k_filename_list`,
/// returning the structured [`ExecProgramValueResult`].
///
/// Structured-output sibling of [`exec_program`]. Parses the files via
/// `kcl_parser::load_program`, applies any overrides, then delegates to
/// [`execute_to_value`] for resolution + evaluation. The returned result
/// holds a [`kcl_runtime::ValueRef`] instead of JSON+YAML strings;
/// consumers walk the tree directly (typically via codegen-emitted
/// `TryFrom<&ValueRef>` impls — see the rust-codegen crate, Phase 4).
///
/// **Note that it is not thread safe.**
pub fn exec_program_to_value(
    sess: ParseSessionRef,
    args: &ExecProgramArgs,
) -> Result<ExecProgramValueResult> {
    let opts = args.get_load_program_options();
    let kcl_paths_str = args
        .k_filename_list
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<&str>>();
    let module_cache = KCLModuleCache::default();
    let mut program = load_program(
        sess.clone(),
        kcl_paths_str.as_slice(),
        Some(opts),
        Some(module_cache),
    )?
    .program;
    apply_overrides(
        &mut program,
        &args.overrides,
        &[],
        args.print_override_ast || args.debug > 0,
    )?;
    execute_to_value(sess, program, args)
}

/// Resolve and evaluate an already-parsed KCL [`Program`], returning the
/// structured [`ExecProgramValueResult`].
///
/// Structured-output sibling of [`execute`]. Behaves identically up to the
/// final evaluation step, where [`FastRunner::run_to_value`] returns a
/// [`kcl_runtime::ValueRef`] instead of JSON+YAML strings. The
/// `compile_only` short-circuit returns an empty result with
/// `value = ValueRef::undefined()`.
///
/// **Note that it is not thread safe.**
pub fn execute_to_value(
    sess: ParseSessionRef,
    mut program: Program,
    args: &ExecProgramArgs,
) -> Result<ExecProgramValueResult> {
    if args.compile_only {
        let resolve_opts = Options {
            merge_program: false,
            ..Default::default()
        };
        let scope = resolve_program_with_opts(&mut program, resolve_opts, None);
        emit_compile_diag_to_string(sess, &scope, args.compile_only)?;
        return Ok(ExecProgramValueResult::default());
    }
    let scope = resolve_program(&mut program);
    emit_compile_diag_to_string(sess, &scope, false)?;
    FastRunner::new(Some(RunnerOptions {
        plugin_agent_ptr: args.plugin_agent,
    }))
    .run_to_value(&program, args)
}

/// `execute_module` can directly execute the ast `Module`.
/// `execute_module` constructs `Program` with default pkg name `MAIN_PKG`,
/// and calls method `execute` with default `plugin_agent` and `ExecProgramArgs`.
/// For more information, see doc above method `execute`.
///
/// **Note that it is not thread safe.**
pub fn execute_module(m: Module) -> Result<ExecProgramResult> {
    let mut pkgs = HashMap::new();
    let mut modules = HashMap::new();
    pkgs.insert(MAIN_PKG.to_string(), vec![m.filename.clone()]);
    modules.insert(m.filename.clone(), Arc::new(RwLock::new(m)));

    let prog = Program {
        root: MAIN_PKG.to_string(),
        pkgs,
        modules,
        pkgs_not_imported: HashMap::new(),
        modules_not_imported: HashMap::new(),
    };

    execute(
        ParseSessionRef::default(),
        prog,
        &ExecProgramArgs::default(),
    )
}

// [`emit_compile_diag_to_string`] will emit compile diagnostics to string, including parsing and resolving diagnostics.
fn emit_compile_diag_to_string(
    sess: ParseSessionRef,
    scope: &ProgramScope,
    include_warnings: bool,
) -> Result<()> {
    let mut res_str = sess.1.write().emit_to_string()?;
    let sema_err = scope.emit_diagnostics_to_string(sess.0.clone(), include_warnings);
    if let Err(err) = &sema_err {
        #[cfg(not(target_os = "windows"))]
        res_str.push('\n');
        #[cfg(target_os = "windows")]
        res_str.push_str("\r\n");
        res_str.push_str(err);
    }

    if res_str.is_empty() {
        Ok(())
    } else {
        bail!(res_str)
    }
}
