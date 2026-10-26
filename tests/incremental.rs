use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_typage"))
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap()
}

fn build(root: &Path) -> String {
    let result = run(root, &["build", "--explain"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn tracks_absolute_dynamic_and_binary_dependencies_across_cache_hits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let init = Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap();
    assert!(init.status.success());
    let page = root.join("content/about.typ");
    let mut body = fs::read_to_string(&page).unwrap();
    body.push_str("\n#include \"/content/_partial.typ\"\n#let filename = \"/content/data.bin\"\n#repr(read(filename, encoding: none).at(0))\n");
    fs::write(page, body).unwrap();
    let partial = root.join("content/_partial.typ");
    let binary = root.join("content/data.bin");
    fs::write(&partial, "Alpha dependency").unwrap();
    fs::write(&binary, [0x80]).unwrap();
    build(root);
    let cached = build(root);
    assert!(!cached.contains("dependency changed"), "{cached}");
    let page = root.join("content/about.typ");
    let body = fs::read_to_string(&page).unwrap();
    fs::write(page, format!("{body}\nOnly this page changed.\n")).unwrap();
    let updated = build(root);
    assert_eq!(
        updated
            .lines()
            .filter(|line| line.contains("rebuild"))
            .count(),
        1,
        "{updated}"
    );
    fs::write(&partial, "Beta dependency").unwrap();
    assert!(build(root).contains("dependency changed"));
    let output = root.join("public/about/index.html");
    assert!(fs::read_to_string(&output)
        .unwrap()
        .contains("Beta dependency"));
    build(root);
    fs::write(&binary, [0x81]).unwrap();
    assert!(build(root).contains("dependency changed"));
    assert!(fs::read_to_string(&output).unwrap().contains("129"));
    fs::remove_file(partial).unwrap();
    assert!(!run(root, &["build"]).status.success());
}
