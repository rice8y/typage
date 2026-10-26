use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_typage"))
        .env("NO_COLOR", "1")
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> String {
    let result = run(root, args);
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn preserves_source_relative_evaluation_across_export_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    let dir = root.join("content/nested");
    fs::create_dir_all(dir.join("partials")).unwrap();
    fs::write(dir.join("data.json"), r#"{"value":"Local data"}"#).unwrap();
    fs::write(dir.join("data.csv"), "Local CSV\n").unwrap();
    fs::write(dir.join("bytes.bin"), [255]).unwrap();
    fs::write(dir.join("image.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#).unwrap();
    fs::write(dir.join("partials/_helper.typ"), "#let check() = assert(json(\"../data.json\").value == \"Local data\")\n#check()\nNested helper\n").unwrap();
    let source = r##"#show: page.with(title: "Source context")
#let prefix = "data"
#let helper = "partials/" + "_helper.typ"
#import helper: check
#check()
#include helper
#{
  let filename = prefix + ".json"
  assert(json(filename).value == "Local data")
  assert(csv(prefix + ".csv").at(0).at(0) == "Local CSV")
  assert(read("bytes" + ".bin", encoding: none).at(0) == 255)
  assert(read("page.typ").contains("#show: page.with"))
  json(filename).value
}
#image("image" + ".svg")
`read("missing.txt")`
// image("missing.svg")
"##;
    fs::write(dir.join("page.typ"), source).unwrap();
    let config_path = root.join("config.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(config_path, format!("{config}\n[[pdf_documents]]\npath = \"print.pdf\"\ntemplate = \"print.typ\"\npages = [\"nested/page.typ\"]\n")).unwrap();
    success(root, &["build", "--pdf"]);
    for path in ["public/nested/page/index.pdf", "public/print.pdf"] {
        assert!(fs::read(root.join(path)).unwrap().starts_with(b"%PDF-"));
    }
    let output = root.join("public/nested/page/index.html");
    assert!(fs::read_to_string(&output).unwrap().contains("Local data"));
    let cached = success(root, &["build", "--pdf", "--explain"]);
    assert!(!cached.contains("rebuild"), "{cached}");
    fs::write(dir.join("partials/_helper.typ"), "#let check() = assert(json(\"../data.json\").value == \"Local data\")\n#check()\nChanged helper\n").unwrap();
    let changed = success(root, &["build", "--pdf", "--explain"]);
    assert!(changed.contains("dependency changed"), "{changed}");
    assert!(
        changed.contains("rebuild pdf document print.pdf"),
        "{changed}"
    );
    assert!(fs::read_to_string(&output)
        .unwrap()
        .contains("Changed helper"));
    success(root, &["bundle"]);
    assert!(fs::read_to_string(&output)
        .unwrap()
        .contains("Changed helper"));
    assert_eq!(fs::read_to_string(dir.join("page.typ")).unwrap(), source);
    fs::write(
        dir.join("page.typ"),
        format!("{source}\n#panic(\"source diagnostic\")\n"),
    )
    .unwrap();
    let failed = run(root, &["build"]);
    assert!(!failed.status.success());
    let error = String::from_utf8_lossy(&failed.stderr);
    assert!(error.contains("content/nested/page.typ:"), "{error}");
    assert!(error.contains("source diagnostic"), "{error}");
}
