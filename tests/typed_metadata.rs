use std::{fs, path::Path, process::Command};

fn run(root: &Path, command: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_typage"))
        .args([command, "--root"])
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn evaluates_imported_schemas_defaults_and_native_template_values() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    fs::create_dir_all(root.join("content/projects")).unwrap();
    fs::write(root.join("content/_schema.typ"), r#"
#let shared = (title: str, caption: content, accent: color, date: datetime,
  size: length, enabled: bool, formatter: function, options: union(object((count: int)), object((label: str))),
  rank: value => type(value) == int and value > 0,
  payload: bytes, pattern: regex, release: version, typeval: type, moduleval: module,
  auto_value: auto, none_value: none,
  category: optional(str, default: "package"))
"#).unwrap();
    fs::write(
        root.join("content/config.typ"),
        r#"
#import "_schema.typ": shared
#let collections = (projects: collection.with(schema: shared))
"#,
    )
    .unwrap();
    let values = root.join("content/_values.typ");
    fs::write(
        &values,
        "#let title = \"Native values\"\n#let format(value) = [Formatted #value]\n",
    )
    .unwrap();
    let source = r##"#import "/content/_values.typ" as values
#show: project.with(
  title: values.title, template: "native.typ", caption: [Rich *content*],
  accent: rgb("#123456"), date: datetime(year: 2020, month: 1, day: 2),
  size: 12pt, enabled: true, formatter: values.format,
  options: (count: 3), rank: 1,
  payload: bytes((0, 255)), pattern: regex("abc"), release: version(1, 2, 3),
  typeval: str, moduleval: values, auto_value: auto, none_value: none,
)
Native body.
"##;
    let page = root.join("content/projects/native.typ");
    fs::write(&page, source).unwrap();
    fs::write(
        root.join("templates/native.typ"),
        r#"
#import "@local/typage:VERSION" as typage
#let render(site: (:), page: (:), pages: (), taxonomies: (:), body) = {
  let summary = pages.find(p => p.title == page.title)
  for item in (page, typage.current, summary) {
    assert(type(item.caption) == content)
    assert(type(item.accent) == color)
    assert(type(item.date) == datetime)
    assert(type(item.size) == length)
    assert(type(item.formatter) == function)
    assert(type(item.payload) == bytes and item.payload.at(1) == 255)
    assert(type(item.pattern) == regex)
    assert(type(item.release) == version)
    assert(item.typeval == str and type(item.moduleval) == module)
    assert(item.auto_value == auto and item.none_value == none)
    assert(item.category == "package")
    (item.formatter)(item.caption)
  }
  page.title
  body
}
"#
        .replace("VERSION", env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    let result = run(root, "build");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(run(root, "doctor").status.success());
    let output = root.join("public/projects/native/index.html");
    assert!(fs::read_to_string(&output).unwrap().contains("Formatted"));
    let cached = Command::new(env!("CARGO_BIN_EXE_typage"))
        .args(["build", "--explain", "--root"])
        .arg(root)
        .output()
        .unwrap();
    assert!(cached.status.success());
    assert!(!String::from_utf8_lossy(&cached.stdout).contains("rebuild"));
    fs::write(
        &values,
        "#let title = \"Changed native values\"\n#let format(value) = [Updated #value]\n",
    )
    .unwrap();
    assert!(run(root, "build").status.success());
    assert!(fs::read_to_string(&output).unwrap().contains("Updated"));
    for bad in [
        source.replace("count: 3", "count: true"),
        source.replace("rank: 1", "rank: 0"),
    ] {
        fs::write(&page, bad).unwrap();
        for command in ["build", "doctor"] {
            let result = run(root, command);
            assert!(!result.status.success());
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(error.contains("native.typ"), "{error}");
            assert!(error.contains("does not match its schema"), "{error}");
        }
    }
}
