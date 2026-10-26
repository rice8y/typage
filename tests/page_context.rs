use std::{fs, path::Path, process::Command};

fn success(root: &Path, args: &[&str]) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_typage"))
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn write(root: &Path, path: &str, text: &str) {
    fs::write(
        root.join(path),
        text.replace("VERSION", env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
}

#[test]
fn isolates_current_in_bodies_helpers_templates_and_deferred_content() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    fs::create_dir_all(root.join("content/samples")).unwrap();
    write(
        root,
        "content/config.typ",
        r#"#let collections = (samples: collection.with(schema: (title: str, caption: content, accent: color, formatter: function)))"#,
    );
    write(
        root,
        "content/_shared.typ",
        r#"
#import "@local/typage:VERSION": current
#let check(expected) = {
  assert(current.title == expected, message: "helper current: " + current.title)
  assert(type(current.caption) == content)
  assert(type(current.accent) == color)
  assert((current.formatter)("value") == "native value")
}
#let deferred(expected) = context {
  check(expected)
  text(current.caption)
  read("shared" + ".txt")
}
"#,
    );
    write(root, "content/shared.txt", "Shared dependency");
    for title in ["Alpha", "Beta"] {
        // Exercise the file-backed current transport as well as inline inputs.
        let description = if title == "Beta" {
            "x".repeat(40_000)
        } else {
            "small".into()
        };
        write(
            root,
            &format!("content/samples/{}.typ", title.to_lowercase()),
            &format!(
                r#"
#import "@local/typage:VERSION": current
#show: page.with(title: "{title}", description: "{description}", caption: [Native {title}], accent: red, formatter: value => "native " + value)
#import "../_shared.typ": check, deferred
#assert(current.title == "{title}", message: "body current: " + current.title)
#assert(current.description == "{description}")
#check("{title}")
#deferred("{title}")
#context {{
  assert(current.title == "{title}")
  if target() == "html" [HTML {title}] else [PDF {title}]
}}
"#
            ),
        );
    }
    let template = r#"
#import "@local/typage:VERSION": current
#let render(site: (:), page: (:), pages: (), taxonomies: (:), body) = context {
  assert(current.url == page.url, message: "template current: " + current.url + " != " + page.url)
  assert(current.canonical_url == page.canonical_url)
  [Template #current.title]
  body
}
"#;
    write(root, "templates/base.typ", template);
    write(root, "templates/list.typ", template);
    write(
        root,
        "templates/print.typ",
        r#"
#import "@local/typage:VERSION": current
#let render(site: (:), document: (:), pages: (), body) = {
  assert(current.title == document.title)
  assert(pages.map(p => p.title) == ("Beta", "Alpha"))
  for item in pages { assert(type(item.caption) == content) }
  body
}
"#,
    );
    let config = fs::read_to_string(root.join("config.toml")).unwrap();
    write(root, "config.toml", &format!("{config}\n[[pdf_documents]]\npath = \"print.pdf\"\ntitle = \"Combined\"\ntemplate = \"print.typ\"\npages = [\"samples/beta.typ\", \"samples/alpha.typ\"]\n"));
    success(root, &["build", "--pdf", "--jobs", "4"]);
    assert!(fs::read(root.join("public/print.pdf"))
        .unwrap()
        .starts_with(b"%PDF-"));
    let mut html = Vec::new();
    for name in ["alpha", "beta"] {
        let output = format!("public/samples/{name}/index.html");
        html.push((
            output.clone(),
            fs::read_to_string(root.join(output)).unwrap(),
        ));
        assert!(
            fs::read(root.join(format!("public/samples/{name}/index.pdf")))
                .unwrap()
                .starts_with(b"%PDF-")
        );
    }
    let cached = success(root, &["build", "--pdf", "--explain"]);
    assert!(!cached.contains("rebuild"), "{cached}");
    success(root, &["bundle"]);
    for (path, expected) in html {
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), expected);
    }
    write(root, "content/shared.txt", "Updated dependency");
    let rebuilt = success(root, &["build", "--pdf", "--explain"]);
    assert!(
        rebuilt.contains("rebuild pdf document print.pdf"),
        "{rebuilt}"
    );
    assert!(rebuilt.contains("dependency changed"), "{rebuilt}");
    assert!(
        fs::read_to_string(root.join("public/samples/beta/index.html"))
            .unwrap()
            .contains("Updated dependency")
    );
}
