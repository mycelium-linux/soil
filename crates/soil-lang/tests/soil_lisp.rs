// SPDX-License-Identifier: MIT OR Apache-2.0
//! One Rust harness for tests written in Soil.
//!
//! Each `tests/soil/*.soil` file may define functions named `test/...`.
//! The harness loads the common prelude/bindings, calls every test, and treats
//! any nil/false result as a failure.

use std::path::{Path, PathBuf};

use soil_lang::sexp::Value;
use soil_lang::{NativeHost, Runtime, parse_document};

fn soil_tests() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/soil");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("tests/soil")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("soil"))
        .collect();
    files.sort();
    files
}

fn runtime(root: &Path) -> Runtime {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("hello.txt"), b"hello from vfs\n").unwrap();
    let host = NativeHost::open(&workspace, root, "soil").unwrap();
    let mut runtime = Runtime::new(Box::new(host));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in [
        "stdlib/prelude.soil",
        "stdlib/ui.soil",
        "stdlib/highlight.soil",
        "bindings/soil-time.soil",
        "bindings/soil-records.soil",
        "boot/boot.soil",
    ] {
        runtime.load_file(dir.join(file)).unwrap();
    }
    runtime
}

fn test_names(source: &str) -> Vec<String> {
    parse_document(source)
        .expect("parse Soil test")
        .forms
        .into_iter()
        .filter_map(|form| {
            let items = form.value.as_list()?;
            if items.first().and_then(Value::as_symbol) != Some("defun") {
                return None;
            }
            let name = items.get(1).and_then(Value::as_symbol)?;
            name.starts_with("test/").then(|| name.to_string())
        })
        .collect()
}

#[test]
fn soil_test_files_pass() {
    for file in soil_tests() {
        let source = std::fs::read_to_string(&file).unwrap();
        let names = test_names(&source);
        assert!(
            !names.is_empty(),
            "{} has no test/... defun",
            file.display()
        );

        let root = tempfile::tempdir().unwrap();
        let mut runtime = runtime(root.path());
        runtime
            .load_file(&file)
            .unwrap_or_else(|error| panic!("load {}: {error:#}", file.display()));

        for name in names {
            let result = runtime
                .call(&name, &[])
                .unwrap_or_else(|error| panic!("{}::{name}: {error:#}", file.display()));
            assert!(
                !matches!(result, Value::Nil | Value::Bool(false)),
                "{}::{name} returned {result}",
                file.display()
            );
        }
    }
}
