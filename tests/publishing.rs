use std::{fs, path::Path, process::Command};

fn build(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_typage"))
        .args(["build", "--keep-going", "--root"])
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn compilation_failure_preserves_published_files_and_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    assert!(build(root).status.success());
    let output = root.join("public/about/index.html");
    let cache = root.join(".typage/build-cache.json");
    let previous = fs::read(&output).unwrap();
    let previous_cache = fs::read(&cache).unwrap();
    let source = root.join("content/about.typ");
    let mut body = fs::read_to_string(&source).unwrap();
    body.push_str("\nUnpublished change\n");
    fs::write(source, body).unwrap();
    fs::write(
        root.join("content/broken.typ"),
        "#show: page.with(title: \"Broken\")\n#undefined-function()\n",
    )
    .unwrap();
    fs::write(root.join("static/unpublished.txt"), "Not yet").unwrap();
    assert!(!build(root).status.success());
    assert_eq!(fs::read(output).unwrap(), previous);
    assert_eq!(fs::read(cache).unwrap(), previous_cache);
    assert!(!root.join("public/unpublished.txt").exists());
}
