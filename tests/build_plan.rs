use std::{fs, process::Command};

#[test]
fn all_build_entrypoints_share_template_directory_and_pdf_validation() {
    for case in ["template", "content", "pdf", "traversal"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
            .arg("init")
            .arg(root)
            .output()
            .unwrap()
            .status
            .success());
        let config = root.join("config.toml");
        let mut cfg: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        let expected = match case {
            "template" => {
                fs::remove_file(root.join("templates/base.typ")).unwrap();
                "base.typ"
            }
            "content" => {
                cfg["content_dir"] = "missing-content".into();
                "missing content dir"
            }
            "pdf" => {
                cfg.as_table_mut().unwrap().insert(
                    "pdf_documents".into(),
                    toml::Value::Array(vec![toml::Value::Table(toml::map::Map::from_iter([
                        ("path".into(), "print.pdf".into()),
                        ("template".into(), "print.typ".into()),
                        (
                            "sections".into(),
                            toml::Value::Array(vec!["missing-section".into()]),
                        ),
                    ]))]),
                );
                "missing-section"
            }
            _ => {
                cfg["out_dir"] = "../outside".into();
                "invalid directory configuration"
            }
        };
        fs::write(config, toml::to_string(&cfg).unwrap()).unwrap();
        for command in ["doctor", "build", "bundle"] {
            let result = Command::new(env!("CARGO_BIN_EXE_typage"))
                .args([command, "--root"])
                .arg(root)
                .output()
                .unwrap();
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success(), "{case}/{command}");
            assert!(error.contains(expected), "{case}/{command}: {error}");
            assert!(!root.join("public/index.html").exists());
        }
    }
}

#[test]
fn bundle_exports_a_valid_shared_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    let result = Command::new(env!("CARGO_BIN_EXE_typage"))
        .args(["bundle", "--root"])
        .arg(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for relative in [
        "index.html",
        "about/index.html",
        "posts/hello/index.html",
        "style.css",
    ] {
        assert!(root.join("public").join(relative).is_file(), "{relative}");
    }
}
