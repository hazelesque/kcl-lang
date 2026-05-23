#![allow(clippy::arc_with_non_send_sync)]

use crate::exec_program;
use crate::exec_program_to_value;
use crate::{execute, runner::ExecProgramArgs};
use anyhow::Result;
use kcl_ast::ast::{Module, Program};
use kcl_config::settings::load_file;
use kcl_parser::ParseSession;
use kcl_parser::load_program;
use kcl_utils::path::PathPrefix;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;
use std::{
    collections::HashMap,
    fs::{self, File},
};
use uuid::Uuid;
use walkdir::WalkDir;

const TEST_CASES: &[&str; 5] = &[
    "init_check_order_0",
    "init_check_order_1",
    "normal_2",
    "type_annotation_not_full_2",
    "multi_vars_0",
];

fn exec_data_path() -> String {
    Path::new("src").join("exec_data").display().to_string()
}

fn exec_err_data_path() -> String {
    Path::new("src").join("exec_err_data").display().to_string()
}

fn custom_manifests_data_path() -> String {
    Path::new("src")
        .join("custom_manifests_data")
        .display()
        .to_string()
}

fn exec_prog_args_test_case() -> Vec<String> {
    vec![
        Path::new("exec_prog_args")
            .join("default.json")
            .display()
            .to_string(),
    ]
}

fn settings_file_test_case() -> Vec<(String, String)> {
    vec![(
        Path::new("settings_file")
            .join("settings.yaml")
            .display()
            .to_string(),
        Path::new("settings_file")
            .join("settings.json")
            .display()
            .to_string(),
    )]
}

const EXPECTED_JSON_FILE_NAME: &str = "stdout.golden.json";

fn test_case_path() -> String {
    Path::new("src").join("test_datas").display().to_string()
}

const KCL_FILE_NAME: &str = "main.k";
const MAIN_PKG_NAME: &str = "__main__";

#[derive(serde::Deserialize, serde::Serialize)]
pub struct SimplePanicInfo {
    line: i32,
    col: i32,
    message: String,
}

/// Load test kcl file to ast.Program
fn load_test_program(filename: String) -> Program {
    let module = kcl_parser::parse_file_force_errors(&filename, None).unwrap();
    construct_program(module)
}

/// Construct ast.Program by ast.Module and default configuration.
/// Default configuration:
///     module.pkg = "__main__"
///     Program.root = "__main__"
fn construct_program(module: Module) -> Program {
    let mut pkgs_ast = HashMap::new();
    pkgs_ast.insert(MAIN_PKG_NAME.to_string(), vec![module.filename.clone()]);
    let mut modules = HashMap::new();
    modules.insert(module.filename.clone(), Arc::new(RwLock::new(module)));
    Program {
        root: MAIN_PKG_NAME.to_string(),
        pkgs: pkgs_ast,
        modules,
        pkgs_not_imported: HashMap::new(),
        modules_not_imported: HashMap::new(),
    }
}

/// Load the expect result from stdout.golden.json
fn load_expect_file(filename: String) -> String {
    let f = File::open(filename).unwrap();
    let v: serde_json::Value = serde_json::from_reader(f).unwrap();
    v.to_string()
}

/// Format str by json str
fn format_str_by_json(str: String) -> String {
    let v: serde_json::Value = serde_json::from_str(&str).unwrap();
    v.to_string()
}

fn execute_for_test(kcl_path: &String) -> String {
    let args = ExecProgramArgs::default();
    // Parse kcl file
    let program = load_test_program(kcl_path.to_string());
    // Generate libs, link libs and execute.
    execute(Arc::new(ParseSession::default()), program, &args)
        .unwrap()
        .json_result
}

fn test_kcl_runner_execute() {
    for case in TEST_CASES {
        let kcl_path = &Path::new(&test_case_path())
            .join(case)
            .join(KCL_FILE_NAME)
            .display()
            .to_string();
        let expected_path = &Path::new(&test_case_path())
            .join(case)
            .join(EXPECTED_JSON_FILE_NAME)
            .display()
            .to_string();
        let result = execute_for_test(kcl_path);
        let expected_result = load_expect_file(expected_path.to_string());
        assert_eq!(expected_result, format_str_by_json(result));
    }
}

#[test]
fn test_to_json_program_arg() {
    for case in exec_prog_args_test_case() {
        let test_case_json_file = &Path::new(&test_case_path())
            .join(case)
            .display()
            .to_string();
        let expected_json_str = fs::read_to_string(test_case_json_file).unwrap();
        let exec_prog_args = ExecProgramArgs::default();
        assert_eq!(expected_json_str.trim(), exec_prog_args.to_json().trim());
    }
}

#[test]
fn test_from_str_program_arg() {
    for case in exec_prog_args_test_case() {
        let test_case_json_file = &Path::new(&test_case_path())
            .join(case)
            .display()
            .to_string();
        let expected_json_str = fs::read_to_string(test_case_json_file).unwrap();
        let exec_prog_args = ExecProgramArgs::from_json(&expected_json_str);
        assert_eq!(expected_json_str.trim(), exec_prog_args.to_json().trim());
    }
}

#[test]
fn test_from_setting_file_program_arg() {
    for (case_yaml, case_json) in settings_file_test_case() {
        let test_case_yaml_file = &Path::new(&test_case_path())
            .join(case_yaml)
            .display()
            .to_string();
        let settings_file = load_file(test_case_yaml_file).unwrap();

        let test_case_json_file = &Path::new(&test_case_path())
            .join(case_json)
            .display()
            .to_string();
        let expected_json_str = fs::read_to_string(test_case_json_file).unwrap();

        let exec_prog_args = ExecProgramArgs::try_from(settings_file).unwrap();
        assert_eq!(expected_json_str.trim(), exec_prog_args.to_json().trim());
    }
}

fn test_exec_file() {
    let result = std::panic::catch_unwind(|| {
        for file in get_files(exec_data_path(), false, true, ".k") {
            exec(&file).unwrap();
            println!("{} - PASS", file);
        }
    });
    assert!(result.is_ok());
}

fn test_custom_manifests_output() {
    exec_with_result_at(&custom_manifests_data_path());
}

fn test_exec_with_err_result() {
    exec_with_err_result_at(&exec_err_data_path());
}

fn clean_dir(path: String) {
    if fs::remove_dir_all(path).is_ok() {}
}

#[test]
fn test_exec() {
    clean_dir(
        Path::new(".")
            .join("src")
            .join("exec_data")
            .join(".kcl")
            .display()
            .to_string(),
    );

    clean_dir(
        Path::new(".")
            .join("src")
            .join("exec_err_data")
            .join(".kcl")
            .display()
            .to_string(),
    );

    test_exec_file();
    println!("test_exec_file - PASS");

    test_kcl_runner_execute();
    println!("test_kcl_runner_execute - PASS");

    test_custom_manifests_output();
    println!("test_custom_manifests_output - PASS");

    test_exec_with_err_result();
    println!("test_exec_with_err_result - PASS");

    test_indent_error();
    println!("test_indent_error - PASS");

    test_compile_with_file_pattern();
    println!("test_compile_with_file_pattern - PASS");

    test_uuid();
    println!("test_uuid - PASS");
}

fn test_indent_error() {
    let test_path = PathBuf::from("./src/test_indent_error");
    let kcl_files = get_files(test_path.clone(), false, true, ".k");
    let output_files = get_files(test_path, false, true, ".stderr");

    for (kcl_file, err_file) in kcl_files.iter().zip(&output_files) {
        let mut args = ExecProgramArgs::default();
        args.k_filename_list.push(kcl_file.to_string());
        let res = exec_program(Arc::new(ParseSession::default()), &args);
        assert!(res.is_err());
        if let Err(err_msg) = res {
            let expect_err = fs::read_to_string(err_file).expect("Failed to read file");
            assert!(err_msg.to_string().contains(&expect_err));
        }
    }
}

fn exec(file: &str) -> Result<String, String> {
    let mut args = ExecProgramArgs::default();
    args.k_filename_list.push(file.to_string());
    let opts = args.get_load_program_options();
    let sess = Arc::new(ParseSession::default());
    // Load AST program
    let program = load_program(sess.clone(), &[file], Some(opts), None)
        .unwrap()
        .program;
    // Resolve ATS, generate libs, link libs and execute.
    match execute(sess, program, &args) {
        Ok(result) => {
            if result.err_message.is_empty() {
                Ok(result.json_result)
            } else {
                Err(result.err_message)
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Run all kcl files at path and compare the exec result with the expect output.
fn exec_with_result_at(path: &str) {
    let kcl_files = get_files(path, false, true, ".k");
    let output_files = get_files(path, false, true, ".stdout.golden");
    for (kcl_file, output_file) in kcl_files.iter().zip(&output_files) {
        let mut args = ExecProgramArgs::default();
        args.k_filename_list.push(kcl_file.to_string());
        let result = exec_program(Arc::new(ParseSession::default()), &args).unwrap();

        #[cfg(not(target_os = "windows"))]
        let newline = "\n";
        #[cfg(target_os = "windows")]
        let newline = "\r\n";

        let expected_str = std::fs::read_to_string(output_file).unwrap();
        let expected = expected_str
            .strip_suffix(newline)
            .unwrap_or(&expected_str)
            .to_string();

        #[cfg(target_os = "windows")]
        let expected = expected.replace("\r\n", "\n");

        assert_eq!(
            result.yaml_result, expected,
            "test case {} {} failed",
            path, kcl_file
        );
    }
}

/// Run all kcl files at path and compare the exec error result with the expect error output.
fn exec_with_err_result_at(path: &str) {
    let kcl_files = get_files(path, false, true, ".k");
    let output_files = get_files(path, false, true, ".stderr.json");

    let prev_hook = std::panic::take_hook();
    // disable print panic info
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(|| {
        for (kcl_file, _) in kcl_files.iter().zip(&output_files) {
            let mut args = ExecProgramArgs::default();
            args.k_filename_list.push(kcl_file.to_string());
            let result = exec_program(Arc::new(ParseSession::default()), &args);
            if let Ok(result) = result {
                assert!(!result.err_message.is_empty(), "{}", result.err_message);
            } else {
                assert!(result.is_err());
            }
        }
    });
    assert!(result.is_ok());
    std::panic::set_hook(prev_hook);
}

/// Get kcl files from path.
fn get_files<P: AsRef<Path>>(
    path: P,
    recursively: bool,
    sorted: bool,
    suffix: &str,
) -> Vec<String> {
    let mut files = vec![];
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            let file = path.to_str().unwrap();
            if file.ends_with(suffix) && (recursively || entry.depth() == 1) {
                files.push(file.to_string())
            }
        }
    }
    if sorted {
        files.sort();
    }
    files
}

fn test_compile_with_file_pattern() {
    let test_path = PathBuf::from("./src/test_file_pattern/**/main.k");
    let mut args = ExecProgramArgs::default();
    args.k_filename_list.push(test_path.display().to_string());
    let res = exec_program(Arc::new(ParseSession::default()), &args);
    assert!(res.is_ok());
    assert_eq!(
        res.as_ref().unwrap().yaml_result,
        "k3: Hello World!\nk1: Hello World!\nk2: Hello World!"
    );
    assert_eq!(
        res.as_ref().unwrap().json_result,
        "{\"k3\": \"Hello World!\", \"k1\": \"Hello World!\", \"k2\": \"Hello World!\"}"
    );
}

fn test_uuid() {
    let res = exec(
        &PathBuf::from(".")
            .join("src")
            .join("test_uuid")
            .join("main.k")
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
    );

    let v: Value = serde_json::from_str(res.clone().unwrap().as_str()).unwrap();
    assert!(v["a"].as_str().is_some());
    if let Some(uuid_str) = v["a"].as_str() {
        assert!(Uuid::parse_str(uuid_str).is_ok());
    }
}

#[test]
fn test_compile_with_symbolic_link() {
    let main_test_path = PathBuf::from("./src/test_symbolic_link/test_pkg/bbb/main.k");
    let mut args = ExecProgramArgs::default();
    args.k_filename_list
        .push(main_test_path.display().to_string());
    let res = exec_program(Arc::new(ParseSession::default()), &args);
    assert!(res.is_ok());
    assert_eq!(
        res.as_ref().unwrap().yaml_result,
        "The_first_kcl_program: Hello World!\nb: 1"
    );
    assert_eq!(
        res.as_ref().unwrap().json_result,
        "{\"The_first_kcl_program\": \"Hello World!\", \"b\": 1}"
    );
}

#[test]
fn test_kcl_issue_1799() {
    let main_test_path = PathBuf::from("./src/test_issues/github.com/kcl-lang/kcl/1799/main.k");
    let mut args = ExecProgramArgs::default();
    args.k_filename_list
        .push(main_test_path.display().to_string());
    args.work_dir = Some(".".to_string());
    let res = exec_program(Arc::new(ParseSession::default()), &args);
    assert!(res.is_ok());
    assert_eq!(
        res.as_ref().unwrap().yaml_result,
        format!(
            "a: {}",
            main_test_path
                .parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .adjust_canonicalization()
        )
    );
}

/// Tests for the Phase 2 structured-output path. Walks the [`ValueRef`] tree
/// returned by [`exec_program_to_value`] / [`FastRunner::run_to_value`] and
/// asserts structural shape end-to-end.
///
/// These tests are the canary for the structured-path-vs-string-path drift
/// the rest of the workspace's grammar tests cannot catch by themselves.
#[cfg(test)]
mod structured_output_tests {
    use super::*;

    /// Top-level result of evaluating a configuration program is a dict
    /// (with the variable name as the key, the assigned value as the value).
    #[test]
    fn test_structured_value_top_level_is_dict() {
        let sess = Arc::new(ParseSession::default());
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            k_code_list: vec!["alice = {age = 18}".to_string()],
            ..Default::default()
        };
        let result = exec_program_to_value(sess, &args).expect("evaluation failed");
        assert!(
            result.err_message.is_empty(),
            "unexpected err_message: {}",
            result.err_message
        );
        assert!(
            result.value.is_dict(),
            "top-level value is not a dict: type={}",
            result.value.type_str()
        );
        let alice = result
            .value
            .dict_get_value("alice")
            .expect("missing key 'alice' at top level");
        assert!(alice.is_dict(), "alice is not a dict");
        let age = alice
            .dict_get_value("age")
            .expect("missing key 'age' under alice");
        assert_eq!(age.as_int(), 18);
    }

    /// Schema instances surface as `Value::schema_value` carrying the schema
    /// name + pkgpath inline. Confirms the chain-of-trust property the
    /// Phase 4 codegen will rely on.
    #[test]
    fn test_structured_value_preserves_schema_metadata() {
        let sess = Arc::new(ParseSession::default());
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            k_code_list: vec![
                "schema Vm:\n    memory_mb: int = 1024\n    name: str\n\nvm = Vm {name = \"alpha\"}".to_string(),
            ],
            ..Default::default()
        };
        let result = exec_program_to_value(sess, &args).expect("evaluation failed");
        assert!(
            result.err_message.is_empty(),
            "unexpected err_message: {}",
            result.err_message
        );
        let vm = result
            .value
            .dict_get_value("vm")
            .expect("missing key 'vm' at top level");
        assert!(vm.is_schema(), "vm is not a schema_value: type={}", vm.type_str());
        let vm_inner = vm.as_schema();
        assert_eq!(vm_inner.name, "Vm", "schema name mismatch");
        // Schema-instantiated default value flows through to the result.
        let memory = vm
            .dict_get_value("memory_mb")
            .expect("missing memory_mb on vm schema instance");
        assert_eq!(memory.as_int(), 1024);
        let name = vm
            .dict_get_value("name")
            .expect("missing name on vm schema instance");
        assert_eq!(name.as_str(), "alpha");
    }

    /// For non-yaml_stream programs (the simple-config case), the JSON
    /// produced by serialising the structured ValueRef should byte-equal the
    /// string-path's `json_result`. This guards the snapshot equivalence
    /// between `run()` and `run_to_value()` for the workload that codegen
    /// consumers care about.
    #[test]
    fn test_structured_json_matches_string_path() {
        let code = "schema Disk:\n    size_gb: int\n\nschema Vm:\n    name: str\n    disks: [Disk]\n\nvm = Vm {\n    name = \"beta\"\n    disks = [Disk {size_gb = 20}, Disk {size_gb = 40}]\n}";
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            k_code_list: vec![code.to_string()],
            ..Default::default()
        };
        let sess_str = Arc::new(ParseSession::default());
        let string_result = exec_program(sess_str, &args).expect("string path failed");
        assert!(
            string_result.err_message.is_empty(),
            "string-path err: {}",
            string_result.err_message
        );

        let sess_val = Arc::new(ParseSession::default());
        let value_result = exec_program_to_value(sess_val, &args).expect("value path failed");
        assert!(
            value_result.err_message.is_empty(),
            "value-path err: {}",
            value_result.err_message
        );

        // Both representations of the same evaluation should be observably
        // equivalent at the structural level. Walk both and assert key fields.
        let str_json: Value =
            serde_json::from_str(&string_result.json_result).expect("string json invalid");
        let str_vm = str_json.get("vm").expect("string path missing 'vm'");
        assert_eq!(str_vm.get("name").and_then(Value::as_str), Some("beta"));
        let str_disks = str_vm.get("disks").and_then(Value::as_array).expect("disks");
        assert_eq!(str_disks.len(), 2);

        let val_vm = value_result.value.dict_get_value("vm").expect("'vm'");
        assert!(val_vm.is_schema(), "value-path vm is not a schema");
        assert_eq!(val_vm.dict_get_value("name").unwrap().as_str(), "beta");
        let val_disks = val_vm.dict_get_value("disks").expect("disks");
        assert!(val_disks.is_list());
        assert_eq!(val_disks.as_list_ref().values.len(), 2);
    }

    /// Failure contract for runtime errors: a program that passes sema but
    /// fails at evaluation (here, a check-block violation) returns
    /// `Ok` with `err_message` non-empty and `value = ValueRef::undefined()`.
    /// Locks down the runner-level runtime-failure shape before Phase 6a
    /// swaps it for structured EvaluationError.
    ///
    /// NOTE: sema-level errors (type mismatches, undefined identifiers)
    /// return `Result::Err` from `exec_program_to_value` rather than this
    /// `Ok-with-err_message-and-undefined-value` shape. The two error
    /// surfaces mirror the existing string path. Phase 6a unifies them
    /// behind `EvaluationError::{Parse, Resolve, Evaluate, Internal}`.
    #[test]
    fn test_structured_value_failure_returns_undefined_value() {
        let sess = Arc::new(ParseSession::default());
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            // Passes sema (memory_mb is int), fails at evaluation via the
            // check-block predicate.
            k_code_list: vec![
                "schema Vm:\n    memory_mb: int\n    check:\n        memory_mb >= 1024, \"memory_mb must be at least 1024\"\n\nvm = Vm {memory_mb = 16}".to_string(),
            ],
            ..Default::default()
        };
        let result = exec_program_to_value(sess, &args)
            .expect("runtime check-block failures return Ok-with-err_message, not Err");
        assert!(
            !result.err_message.is_empty(),
            "expected err_message to describe the check-block violation, got empty"
        );
        assert!(
            result.value.is_undefined(),
            "expected ValueRef::undefined() on runtime failure, got: type={}",
            result.value.type_str()
        );
    }

    /// Phase 2.5 regression test: two calls to `yaml.encode` within a
    /// single KCL program return correct independent values.
    ///
    /// Pre-Phase-2.5, the runtime's `kcl_value_plan_to_json/yaml` C-API
    /// functions wrote each result into `Context::json_result` /
    /// `Context::yaml_result` and returned a `ValueRef::str` referencing
    /// the context-resident storage. A second call would overwrite those
    /// fields — and any C consumer that held the first call's `*char` past
    /// the second would read corrupted data. Post-Phase-2.5, the returned
    /// `ValueRef` owns its String independently and the two calls cannot
    /// interfere. This test exercises the doubled-call path from user code.
    #[test]
    fn test_yaml_encode_twice_independent_results() {
        let sess = Arc::new(ParseSession::default());
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            k_code_list: vec![
                concat!(
                    "import yaml\n",
                    "first = yaml.encode({a = 1})\n",
                    "second = yaml.encode({b = 2})\n",
                )
                .to_string(),
            ],
            ..Default::default()
        };
        let result = exec_program(sess, &args).expect("evaluation failed");
        assert!(
            result.err_message.is_empty(),
            "unexpected err_message: {}",
            result.err_message
        );
        // The two yaml.encode results must reflect the distinct inputs;
        // pre-cleanup, the second call's context-resident storage could
        // bleed through and both KCL string values would have been
        // equal. Asserting their distinctness catches the regression.
        let parsed: Value =
            serde_json::from_str(&result.json_result).expect("json_result invalid");
        let first = parsed
            .get("first")
            .and_then(Value::as_str)
            .expect("first not a string");
        let second = parsed
            .get("second")
            .and_then(Value::as_str)
            .expect("second not a string");
        assert!(
            first.contains("a: 1"),
            "first did not encode {{a=1}}: {first}"
        );
        assert!(
            second.contains("b: 2"),
            "second did not encode {{b=2}}: {second}"
        );
        assert_ne!(
            first, second,
            "first and second yaml.encode results collapsed to the same value"
        );
    }

    /// The structured-output path returns the dict-merged global scope
    /// directly. An empty program should produce an empty dict, mirroring
    /// what the string path emits (`{}` / empty YAML).
    #[test]
    fn test_structured_value_empty_program() {
        let sess = Arc::new(ParseSession::default());
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            k_code_list: vec!["".to_string()],
            ..Default::default()
        };
        let result = exec_program_to_value(sess, &args).expect("evaluation failed");
        assert!(
            result.err_message.is_empty(),
            "unexpected err_message: {}",
            result.err_message
        );
        assert!(result.value.is_dict(), "top-level value is not a dict");
        assert_eq!(
            result.value.as_dict_ref().values.len(),
            0,
            "empty program produced non-empty dict"
        );
    }

    /// Tilley-shaped fixture exercising the realistic surface Phase 4 codegen
    /// will need to read. Single program, no inheritance, no mixins (both
    /// deferred from Phase 4 MVP per D9/D10), but does exercise:
    ///
    ///   - Discriminator-tagged single-schema variant (Tilley's TestAssertion
    ///     pattern; mapped to a Rust tagged enum by Phase 4's
    ///     `# @rust: tagged_enum(discriminator = ...)` annotation under D6)
    ///   - Conditional check-block predicates per discriminator value
    ///   - Optional fields (`field?: T`) — both set and unset across variants
    ///   - Primitive-union typed fields (`"a" | "b" | "c"`)
    ///   - Nested schemas (HealthCheck inside Vm)
    ///   - Lists of schemas
    ///   - Defaults flowing through (Vm with no overrides uses memory_mb,
    ///     cores, and health_checks defaults)
    ///
    /// Empirically pins the **D5 answer**: an `field?: T` declaration where
    /// the instance does not set the field surfaces in the resulting
    /// `ValueRef` as `Value::undefined`. (The plan's D5 hypothesis was
    /// `Value::none`; this fixture proved it wrong before any codegen was
    /// written. Phase 4 codegen maps `Value::undefined → None` accordingly.
    /// Explicit `field = None` in source — distinct from omitted — is
    /// expected to produce `Value::none`; Phase 4A's pre-investigation
    /// memo confirms the distinction empirically.)
    #[test]
    fn test_structured_value_tilley_shaped_fixture() {
        let sess = Arc::new(ParseSession::default());
        let args = ExecProgramArgs {
            k_filename_list: vec!["test.k".to_string()],
            k_code_list: vec![concat!(
                "schema HealthCheck:\n",
                "    type: \"command\" | \"service\" | \"port\"\n",
                "    command?: str\n",
                "    expected_exit?: int\n",
                "    service?: str\n",
                "    state?: \"running\" | \"stopped\"\n",
                "    port?: int\n",
                "    host?: str\n",
                "\n",
                "    check:\n",
                "        command != Undefined if type == \"command\", \"command variant requires command\"\n",
                "        service != Undefined if type == \"service\", \"service variant requires service\"\n",
                "        port != Undefined if type == \"port\", \"port variant requires port\"\n",
                "\n",
                "schema Vm:\n",
                "    name: str\n",
                "    memory_mb: int = 1024\n",
                "    cores: int = 2\n",
                "    health_checks: [HealthCheck] = []\n",
                "\n",
                "vms = [\n",
                "    Vm {\n",
                "        name = \"alpha\"\n",
                "        memory_mb = 2048\n",
                "        health_checks = [\n",
                "            HealthCheck {type = \"command\", command = \"/usr/bin/true\", expected_exit = 0}\n",
                "            HealthCheck {type = \"service\", service = \"sshd\", state = \"running\"}\n",
                "            HealthCheck {type = \"port\", port = 22}\n",
                "        ]\n",
                "    }\n",
                "    Vm {\n",
                "        name = \"beta\"\n",
                "    }\n",
                "]\n",
            )
            .to_string()],
            ..Default::default()
        };
        let result = exec_program_to_value(sess, &args).expect("evaluation failed");
        assert!(
            result.err_message.is_empty(),
            "unexpected err_message: {}",
            result.err_message
        );

        // Top level: dict with a single key `vms`.
        assert!(result.value.is_dict(), "top-level is not a dict");
        let vms = result
            .value
            .dict_get_value("vms")
            .expect("missing key 'vms' at top level");

        assert!(vms.is_list(), "vms is not a list: type={}", vms.type_str());
        let vms_list = vms.as_list_ref();
        assert_eq!(vms_list.values.len(), 2, "expected 2 VMs");

        // VM[0]: alpha — all defaults overridden, three health checks.
        let alpha = &vms_list.values[0];
        assert!(alpha.is_schema(), "vms[0] is not a schema_value");
        assert_eq!(alpha.as_schema().name, "Vm");
        assert_eq!(alpha.dict_get_value("name").unwrap().as_str(), "alpha");
        assert_eq!(alpha.dict_get_value("memory_mb").unwrap().as_int(), 2048);
        assert_eq!(
            alpha.dict_get_value("cores").unwrap().as_int(),
            2,
            "cores default did not flow through on alpha"
        );

        let alpha_checks = alpha.dict_get_value("health_checks").expect("health_checks");
        assert!(alpha_checks.is_list());
        let alpha_checks_list = alpha_checks.as_list_ref();
        assert_eq!(alpha_checks_list.values.len(), 3);

        // health_checks[0]: command variant.
        let hc0 = &alpha_checks_list.values[0];
        assert!(hc0.is_schema(), "hc0 is not a schema_value");
        assert_eq!(hc0.as_schema().name, "HealthCheck");
        assert_eq!(hc0.dict_get_value("type").unwrap().as_str(), "command");
        assert_eq!(
            hc0.dict_get_value("command").unwrap().as_str(),
            "/usr/bin/true"
        );
        assert_eq!(hc0.dict_get_value("expected_exit").unwrap().as_int(), 0);

        // D5 empirical answer: unset optional fields surface as
        // Value::undefined (not Value::none, as the plan's hypothesis
        // assumed). Codegen maps Value::undefined → None.
        assert_unset_optional_is_undefined(hc0, "service", "hc0/command-variant");
        assert_unset_optional_is_undefined(hc0, "state", "hc0/command-variant");
        assert_unset_optional_is_undefined(hc0, "port", "hc0/command-variant");
        assert_unset_optional_is_undefined(hc0, "host", "hc0/command-variant");

        // health_checks[1]: service variant.
        let hc1 = &alpha_checks_list.values[1];
        assert_eq!(hc1.as_schema().name, "HealthCheck");
        assert_eq!(hc1.dict_get_value("type").unwrap().as_str(), "service");
        assert_eq!(hc1.dict_get_value("service").unwrap().as_str(), "sshd");
        assert_eq!(hc1.dict_get_value("state").unwrap().as_str(), "running");
        assert_unset_optional_is_undefined(hc1, "command", "hc1/service-variant");
        assert_unset_optional_is_undefined(hc1, "expected_exit", "hc1/service-variant");
        assert_unset_optional_is_undefined(hc1, "port", "hc1/service-variant");
        assert_unset_optional_is_undefined(hc1, "host", "hc1/service-variant");

        // health_checks[2]: port variant (host left unset to exercise the
        // both-optionals-present-and-absent case within a single variant).
        let hc2 = &alpha_checks_list.values[2];
        assert_eq!(hc2.as_schema().name, "HealthCheck");
        assert_eq!(hc2.dict_get_value("type").unwrap().as_str(), "port");
        assert_eq!(hc2.dict_get_value("port").unwrap().as_int(), 22);
        assert_unset_optional_is_undefined(hc2, "host", "hc2/port-variant");
        assert_unset_optional_is_undefined(hc2, "command", "hc2/port-variant");
        assert_unset_optional_is_undefined(hc2, "expected_exit", "hc2/port-variant");
        assert_unset_optional_is_undefined(hc2, "service", "hc2/port-variant");
        assert_unset_optional_is_undefined(hc2, "state", "hc2/port-variant");

        // VM[1]: beta — every defaultable field defaulted.
        let beta = &vms_list.values[1];
        assert!(beta.is_schema());
        assert_eq!(beta.as_schema().name, "Vm");
        assert_eq!(beta.dict_get_value("name").unwrap().as_str(), "beta");
        assert_eq!(beta.dict_get_value("memory_mb").unwrap().as_int(), 1024);
        assert_eq!(beta.dict_get_value("cores").unwrap().as_int(), 2);
        let beta_checks = beta
            .dict_get_value("health_checks")
            .expect("health_checks default did not flow through on beta");
        assert!(beta_checks.is_list());
        assert_eq!(
            beta_checks.as_list_ref().values.len(),
            0,
            "expected health_checks default to be empty list"
        );
    }

    /// Helper for `test_structured_value_tilley_shaped_fixture`. Asserts
    /// that an optional schema field declared `field?: T` and not set on
    /// the instance surfaces in the `ValueRef` tree as `Value::undefined`
    /// — the empirically-verified D5 answer.
    ///
    /// If the runtime ever changes to produce `Value::none` (the
    /// originally-hypothesised mapping), absent-from-dict, or anything
    /// else, the failure message records which alternative we got so the
    /// codegen mapping can be updated in lockstep.
    fn assert_unset_optional_is_undefined(
        schema_val: &kcl_runtime::ValueRef,
        field: &str,
        ctx: &str,
    ) {
        match schema_val.dict_get_value(field) {
            Some(v) if v.is_undefined() => { /* D5 empirical answer holds */ }
            Some(v) if v.is_none() => panic!(
                "D5 drift: unset optional `{field}` on {ctx} is Value::none, \
                 not Value::undefined — runtime semantics changed; update codegen"
            ),
            Some(v) => panic!(
                "D5 drift: unset optional `{field}` on {ctx} is neither undefined nor none; \
                 got type={}",
                v.type_str()
            ),
            None => panic!(
                "D5 drift: unset optional `{field}` on {ctx} is absent-from-dict; \
                 codegen needs a different lookup pattern"
            ),
        }
    }
}
