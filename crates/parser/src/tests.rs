use std::{
    env,
    panic::{catch_unwind, set_hook},
    path::Path,
};

use compiler_base_span::{FilePathMapping, SourceMap};
use entry::expand_input_files;
use kcl_config::modfile::{KCL_PKG_PATH, get_vendor_home};

use crate::*;

use core::any::Any;

mod ast;
mod error_recovery;
mod expr;
mod file;
mod types;

#[macro_export]
macro_rules! parse_expr_snapshot {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($crate::tests::parsing_expr_string($src));
        }
    };
}

#[macro_export]
macro_rules! parse_module_snapshot {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($crate::tests::parsing_module_string($src));
        }
    };
}

#[macro_export]
macro_rules! parse_type_snapshot {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($crate::tests::parsing_type_string($src));
        }
    };
}

#[macro_export]
macro_rules! parse_type_node_snapshot {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($crate::tests::parsing_type_node_string($src));
        }
    };
}

#[macro_export]
macro_rules! parse_file_ast_json_snapshot {
    ($name:ident, $filename:expr, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($crate::tests::parsing_file_ast_json($filename, $src));
        }
    };
}

#[macro_export]
macro_rules! parse_file_snapshot {
    ($name:ident, $filename:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($crate::tests::parsing_file_string($filename));
        }
    };
}

pub(crate) fn parsing_expr_string(src: &str) -> String {
    let sm = SourceMap::new(FilePathMapping::empty());
    let sf = sm.new_source_file(PathBuf::from("").into(), src.to_string());
    let sess = &ParseSession::with_source_map(Arc::new(sm));

    match sf.src.as_ref() {
        Some(src_from_sf) => create_session_globals_then(|| {
            let stream = parse_token_streams(sess, src_from_sf.as_str(), new_byte_pos(0));
            let mut parser = Parser::new(sess, stream);
            let expr = parser.parse_expr();
            format!("{expr:#?}\n")
        }),
        None => "".to_string(),
    }
}

pub(crate) fn parsing_module_string(src: &str) -> String {
    let sm = SourceMap::new(FilePathMapping::empty());
    let sf = sm.new_source_file(PathBuf::from("").into(), src.to_string());
    let sess = &ParseSession::with_source_map(Arc::new(sm));

    match sf.src.as_ref() {
        Some(src_from_sf) => create_session_globals_then(|| {
            let stream = parse_token_streams(sess, src_from_sf.as_str(), new_byte_pos(0));
            let mut parser = Parser::new(sess, stream);
            let module = parser.parse_module();
            format!("{module:#?}\n")
        }),
        None => "".to_string(),
    }
}

pub(crate) fn parsing_type_string(src: &str) -> String {
    let sm = SourceMap::new(FilePathMapping::empty());
    sm.new_source_file(PathBuf::from("").into(), src.to_string());
    let sess = &ParseSession::with_source_map(Arc::new(sm));

    create_session_globals_then(|| {
        let stream = parse_token_streams(sess, src, new_byte_pos(0));
        let mut parser = Parser::new(sess, stream);
        let typ = parser.parse_type_annotation();
        format!("{typ:#?}\n")
    })
}

pub(crate) fn parsing_type_node_string(src: &str) -> String {
    let sm = SourceMap::new(FilePathMapping::empty());
    sm.new_source_file(PathBuf::from("").into(), src.to_string());
    let sess = &ParseSession::with_source_map(Arc::new(sm));

    create_session_globals_then(|| {
        let stream = parse_token_streams(sess, src, new_byte_pos(0));
        let mut parser = Parser::new(sess, stream);
        let typ = parser.parse_type_annotation();
        typ.node.to_string()
    })
}

pub(crate) fn parsing_file_ast_json(filename: &str, src: &str) -> String {
    let m = crate::parse_file_with_global_session(
        Arc::new(ParseSession::default()),
        filename,
        Some(src.into()),
    )
    .unwrap();
    serde_json::ser::to_string_pretty(&m).unwrap()
}

pub(crate) fn parsing_file_string(filename: &str) -> String {
    let code = std::fs::read_to_string(filename).unwrap();
    let m = crate::parse_single_file(filename.trim_start_matches("testdata/"), Some(code))
        .expect(filename)
        .module;
    serde_json::ser::to_string_pretty(&m).unwrap()
}

pub fn check_result_panic_info(result: Result<(), Box<dyn Any + Send>>) {
    if let Err(e) = result {
        assert!(e.downcast::<String>().is_ok());
    };
}

const PARSE_EXPR_INVALID_TEST_CASES: &[&str] =
    &["fs1_i1re1~s", "fh==-h==-", "8_________i", "1MM", "0x00x"];

#[test]
pub fn test_parse_expr_invalid() {
    for case in PARSE_EXPR_INVALID_TEST_CASES {
        set_hook(Box::new(|_| {}));
        let result = catch_unwind(|| {
            parse_expr(case);
        });
        check_result_panic_info(result);
    }
}

const PARSE_FILE_INVALID_TEST_CASES: &[&str] = &[
    "a: int",                   // No initial value error
    "a -",                      // Invalid binary expression error
    "a?: int",                  // Invalid optional annotation error
    "if a not is not b: a = 1", // Logic operator error
    "if True:\n  a=1\n b=2",    // Indent error with recovery
    "a[1::::]",                 // List slice error
    "a[1 a]",                   // List index error
    "{a ++ 1}",                 // Config attribute operator error
    "func(a=1,b)",              // Call argument error
    "'${}'",                    // Empty string interpolation error
    "'${a: jso}'",              // Invalid string interpolation format spec error
];

#[test]
pub fn test_parse_file_invalid() {
    for case in PARSE_FILE_INVALID_TEST_CASES {
        let result = parse_file_force_errors("test.k", Some((&case).to_string()));
        assert!(result.is_err(), "case: {case}, result {result:?}");
    }
}

pub fn test_vendor_home() {
    let vendor = &PathBuf::from(".")
        .join("testdata")
        .join("test_vendor")
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
        .adjust_canonicalization();
    unsafe { env::set_var(KCL_PKG_PATH, vendor) };
    assert_eq!(get_vendor_home(), vendor.to_string());
}

fn set_vendor_home() -> String {
    // set env vendor
    let vendor = &PathBuf::from(".")
        .join("testdata")
        .join("test_vendor")
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
        .adjust_canonicalization();
    unsafe { env::set_var(KCL_PKG_PATH, vendor) };
    debug_assert_eq!(get_vendor_home(), vendor.to_string());
    vendor.to_string()
}

#[test]
/// The testing will set environment variables,
/// so can not to execute test cases concurrently.
fn test_in_order() {
    test_import_vendor_by_external_arguments();
    println!("{:?} PASS", "test_import_vendor_by_external_arguments");
    test_import_vendor_without_vendor_home();
    println!("{:?} PASS", "test_import_vendor_without_vendor_home");
    test_import_vendor_without_kclmod();
    println!("{:?} PASS", "test_import_vendor_without_kclmod");
    test_import_vendor();
    println!("{:?} PASS", "test_import_vendor");
    test_import_vendor_with_same_internal_pkg();
    println!("{:?} PASS", "test_import_vendor_with_same_internal_pkg");
    test_import_vendor_without_kclmod_and_same_name();
    println!(
        "{:?} PASS",
        "test_import_vendor_without_kclmod_and_same_name"
    );
    test_vendor_home();
    println!("{:?} PASS", "test_vendor_home");
    test_pkg_not_found_suggestion();
    println!("{:?} PASS", "test_pkg_not_found_suggestion");
}

pub fn test_import_vendor() {
    let module_cache = KCLModuleCache::default();
    let vendor = set_vendor_home();
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));

    let test_cases = [
        ("assign.k", vec!["__main__", "assign", "assign.assign"]),
        (
            "config_expr.k",
            vec!["__main__", "config_expr", "config_expr.config_expr_02"],
        ),
        (
            "nested_vendor.k",
            vec![
                "__main__",
                "nested_vendor",
                "nested_vendor.nested_vendor",
                "vendor_subpkg",
                "vendor_subpkg.sub.sub1",
                "vendor_subpkg.sub.sub2",
                "vendor_subpkg.sub.sub",
                "vendor_subpkg.sub",
            ],
        ),
        (
            "subpkg.k",
            vec![
                "__main__",
                "vendor_subpkg",
                "vendor_subpkg.sub.sub1",
                "vendor_subpkg.sub.sub",
                "vendor_subpkg.sub.sub2",
                "vendor_subpkg.sub",
            ],
        ),
    ];

    let dir = &PathBuf::from(".")
        .join("testdata")
        .join("import_vendor")
        .canonicalize()
        .unwrap();

    let test_fn =
        |test_case_name: &&str, pkgs: &Vec<&str>, module_cache: Option<KCLModuleCache>| {
            let test_case_path = dir
                .join(test_case_name)
                .display()
                .to_string()
                .adjust_canonicalization();
            let m = load_program(sess.clone(), &[&test_case_path], None, module_cache)
                .unwrap()
                .program;
            assert_eq!(m.pkgs.len(), pkgs.len());
            m.pkgs.clone().into_iter().for_each(|(name, modules)| {
                println!("{:?} - {:?}", test_case_name, name);
                assert!(pkgs.contains(&name.as_str()));
                for pkg in pkgs.clone() {
                    if name == pkg {
                        if name == "__main__" {
                            assert_eq!(modules.len(), 1);
                            let module = m.get_module(modules.first().unwrap()).unwrap().unwrap();
                            assert_eq!(module.filename, test_case_path);
                        } else {
                            modules.into_iter().for_each(|module| {
                                let module = m.get_module(&module).unwrap().unwrap();
                                assert!(module.filename.contains(&vendor));
                            });
                        }
                        break;
                    }
                }
            });
        };

    test_cases
        .iter()
        .for_each(|(test_case_name, pkgs)| test_fn(test_case_name, pkgs, None));

    test_cases.iter().for_each(|(test_case_name, pkgs)| {
        test_fn(test_case_name, pkgs, Some(module_cache.clone()))
    });
}

pub fn test_import_vendor_without_kclmod() {
    let vendor = set_vendor_home();
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));

    let test_cases = vec![("import_vendor.k", vec!["__main__", "assign.assign"])];

    let dir = &PathBuf::from(".")
        .join("testdata_without_kclmod")
        .canonicalize()
        .unwrap();

    test_cases.into_iter().for_each(|(test_case_name, pkgs)| {
        let test_case_path = dir
            .join(test_case_name)
            .display()
            .to_string()
            .adjust_canonicalization();
        let m = load_program(sess.clone(), &[&test_case_path], None, None)
            .unwrap()
            .program;
        assert_eq!(m.pkgs.len(), pkgs.len());
        m.pkgs.clone().into_iter().for_each(|(name, modules)| {
            assert!(pkgs.contains(&name.as_str()));
            for pkg in pkgs.clone() {
                if name == pkg {
                    if name == "__main__" {
                        assert_eq!(modules.len(), 1);
                        let module = m.get_module(modules.first().unwrap()).unwrap().unwrap();
                        assert_eq!(module.filename, test_case_path);
                    } else {
                        modules.into_iter().for_each(|module| {
                            let module = m.get_module(&module).unwrap().unwrap();
                            assert!(module.filename.contains(&vendor));
                        });
                    }
                    break;
                }
            }
        });
    });
}

pub fn test_import_vendor_without_vendor_home() {
    unsafe { env::set_var(KCL_PKG_PATH, "") };
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));
    let dir = &PathBuf::from(".")
        .join("testdata")
        .join("import_vendor")
        .canonicalize()
        .unwrap();
    let test_case_path = dir.join("assign.k").display().to_string();
    match load_program(sess.clone(), &[&test_case_path], None, None) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs = [
                "pkgpath assign not found in the program",
                "try 'kcl mod add assign' to download the missing package",
                "browse more packages at 'https://artifacthub.io'",
                "pkgpath assign.assign not found in the program",
            ];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }

    match load_program(
        sess.clone(),
        &[&test_case_path],
        None,
        Some(KCLModuleCache::default()),
    ) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs = [
                "pkgpath assign not found in the program",
                "try 'kcl mod add assign' to download the missing package",
                "browse more packages at 'https://artifacthub.io'",
                "pkgpath assign.assign not found in the program",
            ];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }
}

fn test_import_vendor_with_same_internal_pkg() {
    set_vendor_home();
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));
    let dir = &PathBuf::from(".")
        .join("testdata")
        .join("import_vendor")
        .canonicalize()
        .unwrap();
    let test_case_path = dir.join("same_name.k").display().to_string();
    match load_program(sess.clone(), &[&test_case_path], None, None) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs = [
                "the `same_vendor` is found multiple times in the current package and vendor package",
            ];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }
    match load_program(
        sess.clone(),
        &[&test_case_path],
        None,
        Some(KCLModuleCache::default()),
    ) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs = [
                "the `same_vendor` is found multiple times in the current package and vendor package",
            ];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }
}

fn test_import_vendor_without_kclmod_and_same_name() {
    set_vendor_home();
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));
    let dir = &PathBuf::from(".")
        .join("testdata_without_kclmod")
        .join("same_name")
        .canonicalize()
        .unwrap();
    let test_case_path = dir.join("assign.k").display().to_string();
    match load_program(sess.clone(), &[&test_case_path], None, None) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs =
                ["the `assign` is found multiple times in the current package and vendor package"];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }

    match load_program(
        sess.clone(),
        &[&test_case_path],
        None,
        Some(KCLModuleCache::default()),
    ) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs =
                ["the `assign` is found multiple times in the current package and vendor package"];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }
}

fn test_import_vendor_by_external_arguments() {
    let vendor = set_vendor_home();
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));
    let module_cache = KCLModuleCache::default();
    let external_dir = &PathBuf::from(".")
        .join("testdata")
        .join("test_vendor")
        .canonicalize()
        .unwrap();

    let test_cases = [
        (
            "import_by_external_assign.k",
            "assign",
            vec!["__main__", "assign"],
        ),
        (
            "import_by_external_config_expr.k",
            "config_expr",
            vec!["__main__", "config_expr"],
        ),
        (
            "import_by_external_nested_vendor.k",
            "nested_vendor",
            vec![
                "__main__",
                "nested_vendor",
                "vendor_subpkg",
                "vendor_subpkg.sub.sub2",
                "vendor_subpkg.sub.sub1",
                "vendor_subpkg.sub.sub",
                "vendor_subpkg.sub",
            ],
        ),
        (
            "import_by_external_vendor_subpkg.k",
            "vendor_subpkg",
            vec![
                "__main__",
                "vendor_subpkg",
                "vendor_subpkg.sub.sub1",
                "vendor_subpkg.sub.sub2",
                "vendor_subpkg.sub.sub",
                "vendor_subpkg.sub",
            ],
        ),
    ];

    let dir = &PathBuf::from(".")
        .join("testdata_without_kclmod")
        .canonicalize()
        .unwrap();

    let test_fn = |test_case_name: &&str,
                   dep_name: &&str,
                   pkgs: &Vec<&str>,
                   module_cache: Option<KCLModuleCache>| {
        let mut opts = LoadProgramOptions::default();
        opts.package_maps.insert(
            dep_name.to_string(),
            external_dir.join(dep_name).display().to_string(),
        );
        let test_case_path = dir
            .join(test_case_name)
            .display()
            .to_string()
            .adjust_canonicalization();
        let m = load_program(sess.clone(), &[&test_case_path], None, module_cache)
            .unwrap()
            .program;
        assert_eq!(m.pkgs.len(), pkgs.len());
        m.pkgs.clone().into_iter().for_each(|(name, modules)| {
            assert!(pkgs.contains(&name.as_str()));
            for pkg in pkgs.clone() {
                if name == pkg {
                    if name == "__main__" {
                        assert_eq!(modules.len(), 1);
                        let module = m.get_module(modules.first().unwrap()).unwrap().unwrap();
                        assert_eq!(module.filename, test_case_path);
                    } else {
                        modules.into_iter().for_each(|module| {
                            let module = m.get_module(&module).unwrap().unwrap();
                            assert!(module.filename.contains(&vendor));
                        });
                    }
                    break;
                }
            }
        });
    };

    test_cases
        .iter()
        .for_each(|(test_case_name, dep_name, pkgs)| test_fn(test_case_name, dep_name, pkgs, None));

    test_cases
        .iter()
        .for_each(|(test_case_name, dep_name, pkgs)| {
            test_fn(test_case_name, dep_name, pkgs, Some(module_cache.clone()))
        });
}

#[test]
fn test_get_compile_entries_from_paths() {
    let testpath = PathBuf::from("./src/testdata/multimods")
        .canonicalize()
        .unwrap();

    // [`kcl1_path`] is a normal path of the package [`kcl1`] root directory.
    // It looks like `/xxx/xxx/xxx`.
    let kcl1_path = testpath.join("kcl1");

    // [`kcl2_path`] is a mod relative path of the packege [`kcl2`] root directory.
    // It looks like `${kcl2:KCL_MOD}/xxx/xxx`
    let kcl2_path = PathBuf::from("${kcl2:KCL_MOD}/main.k");

    // [`kcl3_path`] is a mod relative path of the [`__main__`] packege.
    let kcl3_path = PathBuf::from("${KCL_MOD}/main.k");

    // [`package_maps`] is a map to show the real path of the mod relative path [`kcl2`].
    let mut opts = LoadProgramOptions::default();
    opts.package_maps.insert(
        "kcl2".to_string(),
        testpath.join("kcl2").to_str().unwrap().to_string(),
    );

    // [`get_compile_entries_from_paths`] will return the map of package name to package root real path.
    let entries = get_compile_entries_from_paths(
        &[
            kcl1_path.to_str().unwrap().to_string(),
            kcl2_path.display().to_string(),
            kcl3_path.display().to_string(),
        ],
        &opts,
    )
    .unwrap();

    assert_eq!(entries.len(), 3);

    assert_eq!(entries.get_nth_entry(0).unwrap().name(), "__main__");
    assert_eq!(
        PathBuf::from(entries.get_nth_entry(0).unwrap().path())
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
        kcl1_path.canonicalize().unwrap().to_str().unwrap()
    );

    assert_eq!(entries.get_nth_entry(1).unwrap().name(), "kcl2");
    assert_eq!(
        PathBuf::from(entries.get_nth_entry(1).unwrap().path())
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
        testpath
            .join("kcl2")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );

    assert_eq!(entries.get_nth_entry(2).unwrap().name(), "__main__");
    assert_eq!(
        PathBuf::from(entries.get_nth_entry(2).unwrap().path())
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap(),
        kcl1_path.canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn test_dir_with_k_code_list() {
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));
    let testpath = PathBuf::from("./src/testdata/test_k_code_list")
        .canonicalize()
        .unwrap();

    let opts = LoadProgramOptions {
        k_code_list: vec!["test_code = 1".to_string()],
        ..Default::default()
    };

    match load_program(
        sess.clone(),
        &[&testpath.display().to_string()],
        Some(opts.clone()),
        None,
    ) {
        Ok(_) => panic!("unreachable code"),
        Err(err) => assert!(err.to_string().contains("Invalid code list")),
    }

    match load_program(
        sess.clone(),
        &[&testpath.display().to_string()],
        Some(opts),
        Some(KCLModuleCache::default()),
    ) {
        Ok(_) => panic!("unreachable code"),
        Err(err) => assert!(err.to_string().contains("Invalid code list")),
    }
}

pub fn test_pkg_not_found_suggestion() {
    let sm = SourceMap::new(FilePathMapping::empty());
    let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));
    let dir = &PathBuf::from("./src/testdata/pkg_not_found")
        .canonicalize()
        .unwrap();
    let test_case_path = dir.join("suggestions.k").display().to_string();
    match load_program(sess.clone(), &[&test_case_path], None, None) {
        Ok(_) => {
            let errors = sess.classification().0;
            let msgs = [
                "pkgpath k9s not found in the program",
                "try 'kcl mod add k9s' to download the missing package",
                "browse more packages at 'https://artifacthub.io'",
            ];
            assert_eq!(errors.len(), msgs.len());
            for (diag, m) in errors.iter().zip(msgs.iter()) {
                assert_eq!(diag.messages[0].message, m.to_string());
            }
        }
        Err(_) => {
            panic!("Unreachable code.")
        }
    }
}

#[test]
fn test_expand_input_files_with_kcl_mod() {
    let path = PathBuf::from("testdata/expand_file_pattern");
    let input_files = vec![
        path.join("**").join("main.k").to_string_lossy().to_string(),
        "${KCL_MOD}/testdata/expand_file_pattern/KCL_MOD".to_string(),
    ];
    let expected_files = [
        path.join("kcl1/kcl2/main.k").to_string_lossy().to_string(),
        path.join("kcl1/kcl4/main.k").to_string_lossy().to_string(),
        path.join("kcl1/main.k").to_string_lossy().to_string(),
        path.join("kcl3/main.k").to_string_lossy().to_string(),
        path.join("main.k").to_string_lossy().to_string(),
        "${KCL_MOD}/testdata/expand_file_pattern/KCL_MOD".to_string(),
    ];
    let got_paths: Vec<String> = expand_input_files(&input_files)
        .iter()
        .map(|s| s.replace(['/', '\\'], ""))
        .collect();
    let expect_paths: Vec<String> = expected_files
        .iter()
        .map(|s| s.replace(['/', '\\'], ""))
        .collect();
    assert_eq!(got_paths, expect_paths);
}

#[test]
#[cfg(not(windows))]
fn test_expand_input_files() {
    let input_files = vec!["./testdata/expand_file_pattern/**/main.k".to_string()];
    let mut expected_files = vec![
        Path::new("testdata/expand_file_pattern/kcl1/kcl2/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl3/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl1/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl1/kcl4/main.k")
            .to_string_lossy()
            .to_string(),
    ];
    expected_files.sort();
    let mut input = expand_input_files(&input_files);
    input.sort();
    assert_eq!(input, expected_files);

    let input_files = vec![
        "./testdata/expand_file_pattern/kcl1/main.k".to_string(),
        "./testdata/expand_file_pattern/**/main.k".to_string(),
    ];
    let mut expected_files = vec![
        Path::new("testdata/expand_file_pattern/kcl1/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl1/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl1/kcl2/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl1/kcl4/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/kcl3/main.k")
            .to_string_lossy()
            .to_string(),
        Path::new("testdata/expand_file_pattern/main.k")
            .to_string_lossy()
            .to_string(),
    ];
    expected_files.sort();
    let mut input = expand_input_files(&input_files);
    input.sort();
    assert_eq!(input, expected_files);
}

#[test]
fn parse_all_file_under_path() {
    let testpath = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("parse_all_modules");

    let main = testpath.join("a");
    let main = main.to_str().unwrap();
    let helloworld = testpath.join("helloworld_0.0.1");
    let b = testpath.join("b");

    let sess = ParseSessionRef::default();
    let mut opts = LoadProgramOptions {
        vendor_dirs: vec![get_vendor_home()],
        ..Default::default()
    };

    opts.package_maps
        .insert("b".to_string(), b.to_str().unwrap().to_string());
    opts.package_maps.insert(
        "helloworld".to_string(),
        helloworld.to_str().unwrap().to_string(),
    );
    let res = load_all_files_under_paths(sess.clone(), &[main], Some(opts), None).unwrap();

    assert_eq!(res.program.pkgs.keys().len(), 1);
    assert_eq!(res.program.pkgs_not_imported.keys().len(), 3);

    assert_eq!(res.paths.len(), 1);
}

/// Tests for in-memory virtual packages, the loader-side hook the embed
/// API (`crates/embed/`) builds on. See `docs/dev_guide/loader-injection.md`
/// for the design.
///
/// Each test exercises a different facet of `LoadProgramOptions::virtual_packages`
/// + `ModuleCache::source_code` co-population: top-level import of a
/// registered module, transitive import across registered modules,
/// registered-but-unimported (lazy), imported-but-not-registered (existing
/// diagnostic preserved).
#[cfg(test)]
mod virtual_packages {
    use super::*;

    /// Build a `KCLModuleCache` pre-populated with `source_code` entries
    /// for each virtual file referenced in `vps`. The cache key is the
    /// `PathBuf` exactly as provided in `VirtualPackage.files`, matching
    /// what `parse_file`'s lookup will use after `find_packages`
    /// → `is_virtual_pkg` → `PkgFile::new(p.into(), ...)`.
    fn module_cache_with_virtual_sources(
        vps: &HashMap<String, VirtualPackage>,
    ) -> KCLModuleCache {
        let cache = KCLModuleCache::default();
        {
            let mut cache_w = cache.write().unwrap();
            for vp in vps.values() {
                for (path, source) in &vp.files {
                    cache_w
                        .source_code
                        .insert(path.clone(), source.clone());
                }
            }
        }
        cache
    }

    fn dummy_main_file() -> String {
        // `load_program` requires at least one file path. We provide a
        // synthetic main path; the actual source comes from `k_code_list`
        // (pairwise consumption per `get_compile_entries_from_paths`).
        "__virtual_main__.k".to_string()
    }

    /// Top-level `import` of a registered virtual package resolves
    /// successfully; the imported package's modules appear in the
    /// resulting Program's `pkgs` map.
    #[test]
    fn test_main_imports_registered_virtual_module() {
        let sm = SourceMap::new(FilePathMapping::empty());
        let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));

        let mut virtual_packages = HashMap::new();
        virtual_packages.insert(
            "tilley".to_string(),
            VirtualPackage {
                root: PathBuf::from("/__kcl_embed__/tilley"),
                files: vec![(
                    PathBuf::from("/__kcl_embed__/tilley/main.k"),
                    "schema Vm:\n    name: str\n    memory_mb: int = 1024\n".to_string(),
                )],
            },
        );

        let opts = LoadProgramOptions {
            k_code_list: vec!["import tilley\nvm = tilley.Vm {name = \"alpha\"}".to_string()],
            virtual_packages: virtual_packages.clone(),
            ..Default::default()
        };

        let cache = module_cache_with_virtual_sources(&virtual_packages);
        let main = dummy_main_file();
        let result = load_program(sess.clone(), &[&main], Some(opts), Some(cache))
            .expect("load_program failed");

        // The registered virtual package is in the program's pkgs map.
        assert!(
            result.program.pkgs.contains_key("tilley"),
            "expected 'tilley' pkg in result; got keys: {:?}",
            result.program.pkgs.keys().collect::<Vec<_>>()
        );

        // Sanity: no errors. (Sema isn't run here — load_program is just
        // parse+resolve-imports — but the import resolution itself must
        // have produced no "pkgpath not found" diagnostics.)
        let errors = sess.classification().0;
        assert!(
            errors.is_empty(),
            "expected zero parse-stage errors; got: {errors:?}"
        );
    }

    /// Transitive `import`: a registered virtual module's `import`
    /// statements resolve through the same registry. A imports B
    /// (both virtual); main imports A; B's modules appear in the
    /// program too.
    #[test]
    fn test_transitive_virtual_imports() {
        let sm = SourceMap::new(FilePathMapping::empty());
        let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));

        let mut virtual_packages = HashMap::new();
        virtual_packages.insert(
            "tilley_vm".to_string(),
            VirtualPackage {
                root: PathBuf::from("/__kcl_embed__/tilley_vm"),
                files: vec![(
                    PathBuf::from("/__kcl_embed__/tilley_vm/main.k"),
                    "schema VmSpec:\n    cores: int = 2\n".to_string(),
                )],
            },
        );
        virtual_packages.insert(
            "tilley".to_string(),
            VirtualPackage {
                root: PathBuf::from("/__kcl_embed__/tilley"),
                files: vec![(
                    PathBuf::from("/__kcl_embed__/tilley/main.k"),
                    "import tilley_vm\n\nschema Vm:\n    name: str\n    spec: tilley_vm.VmSpec\n"
                        .to_string(),
                )],
            },
        );

        let opts = LoadProgramOptions {
            k_code_list: vec![
                "import tilley\nvm = tilley.Vm {name = \"alpha\", spec = tilley_vm.VmSpec {}}"
                    .to_string(),
            ],
            virtual_packages: virtual_packages.clone(),
            ..Default::default()
        };

        let cache = module_cache_with_virtual_sources(&virtual_packages);
        let main = dummy_main_file();
        let result = load_program(sess.clone(), &[&main], Some(opts), Some(cache))
            .expect("load_program failed");

        assert!(
            result.program.pkgs.contains_key("tilley"),
            "missing 'tilley' pkg"
        );
        assert!(
            result.program.pkgs.contains_key("tilley_vm"),
            "missing transitively-imported 'tilley_vm' pkg; pkgs: {:?}",
            result.program.pkgs.keys().collect::<Vec<_>>()
        );

        let errors = sess.classification().0;
        assert!(
            errors.is_empty(),
            "expected zero parse-stage errors; got: {errors:?}"
        );
    }

    /// A virtual package that's registered but never imported by the
    /// main program does not appear in the result (lazy parsing posture
    /// per D3). No errors fire for it.
    #[test]
    fn test_registered_but_unimported_is_lazy() {
        let sm = SourceMap::new(FilePathMapping::empty());
        let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));

        let mut virtual_packages = HashMap::new();
        virtual_packages.insert(
            "unused_pkg".to_string(),
            VirtualPackage {
                root: PathBuf::from("/__kcl_embed__/unused_pkg"),
                files: vec![(
                    PathBuf::from("/__kcl_embed__/unused_pkg/main.k"),
                    // Deliberately not-quite-broken: parses but would
                    // fail sema if it were ever loaded. Lazy resolution
                    // means it never is.
                    "schema Junk:\n    x: int\n".to_string(),
                )],
            },
        );

        let opts = LoadProgramOptions {
            k_code_list: vec!["a = 1".to_string()],
            virtual_packages: virtual_packages.clone(),
            ..Default::default()
        };

        let cache = module_cache_with_virtual_sources(&virtual_packages);
        let main = dummy_main_file();
        let result = load_program(sess.clone(), &[&main], Some(opts), Some(cache))
            .expect("load_program failed");

        assert!(
            !result.program.pkgs.contains_key("unused_pkg"),
            "unused_pkg should not appear in result; got pkgs: {:?}",
            result.program.pkgs.keys().collect::<Vec<_>>()
        );
        let errors = sess.classification().0;
        assert!(
            errors.is_empty(),
            "registered-but-unimported should not surface errors; got: {errors:?}"
        );
    }

    /// Importing a package name that has no registered virtual package
    /// and no on-disk match preserves the existing "pkgpath not found"
    /// diagnostic — the virtual-packages hook does not silently swallow
    /// the failure case.
    #[test]
    fn test_imported_but_not_registered_preserves_diagnostic() {
        let sm = SourceMap::new(FilePathMapping::empty());
        let sess = Arc::new(ParseSession::with_source_map(Arc::new(sm)));

        let opts = LoadProgramOptions {
            k_code_list: vec!["import never_registered\na = 1".to_string()],
            // virtual_packages deliberately empty: the import should
            // fall through to is_internal_pkg / is_external_pkg
            // failures and produce the "pkgpath not found" diagnostic.
            ..Default::default()
        };
        let main = dummy_main_file();
        // The load itself returns Ok; the diagnostic is recorded on the
        // session.
        let _ = load_program(sess.clone(), &[&main], Some(opts), Some(KCLModuleCache::default()));

        let errors = sess.classification().0;
        let found_pkgpath_not_found = errors.iter().any(|d| {
            d.messages.iter().any(|m| {
                m.message
                    .contains("pkgpath never_registered not found in the program")
            })
        });
        assert!(
            found_pkgpath_not_found,
            "expected 'pkgpath ... not found' diagnostic for unregistered import; \
             got: {errors:?}"
        );
    }
}
