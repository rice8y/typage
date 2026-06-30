use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn typst_available() -> bool {
    let out = match Command::new("typst").arg("--version").output() {
        Ok(out) if out.status.success() => out,
        _ => return false,
    };
    let version = String::from_utf8(out.stdout)
        .ok()
        .and_then(|s| parse_typst_version(&s));
    matches!(version, Some(version) if version >= (0, 15, 0))
}

fn parse_typst_version(output: &str) -> Option<(u64, u64, u64)> {
    output.split_whitespace().find_map(|token| {
        let token = token.strip_prefix('v').unwrap_or(token);
        let mut parts = token.split('.');
        let major = parse_version_component(parts.next()?)?;
        let minor = parse_version_component(parts.next()?)?;
        let patch = parse_version_component(parts.next()?)?;
        Some((major, minor, patch))
    })
}

fn parse_version_component(component: &str) -> Option<u64> {
    let digits = component
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn temp_project(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("typage-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

fn assert_exists(path: &Path) {
    assert!(path.exists(), "expected {} to exist", path.display());
}

#[test]
fn init_and_build_basic_project_with_typst() {
    if !typst_available() {
        eprintln!("skipping integration test because typst is not available");
        return;
    }
    let exe = env!("CARGO_BIN_EXE_typage");
    let project = temp_project("basic");

    let init = Command::new(exe)
        .arg("init")
        .arg(&project)
        .output()
        .expect("run typage init");
    assert!(
        init.status.success(),
        "init failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );

    let deploy = Command::new(exe)
        .arg("deploy")
        .arg("init")
        .arg("github-pages")
        .arg("--root")
        .arg(&project)
        .arg("--cname")
        .arg("example.com")
        .output()
        .expect("run typage deploy init");
    assert!(
        deploy.status.success(),
        "deploy init failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&deploy.stdout),
        String::from_utf8_lossy(&deploy.stderr)
    );
    assert_exists(&project.join(".github/workflows/deploy.yml"));
    assert_exists(&project.join("static/.nojekyll"));
    assert_exists(&project.join("static/CNAME"));

    fs::create_dir_all(project.join("content/projects")).unwrap();
    fs::create_dir_all(project.join("content/legacy")).unwrap();
    fs::write(
        project.join("content/config.typ"),
        r#"#let collections = (
  pages: collection.with(schema: (
    title: str,
    description: optional(str),
    template: optional(str),
    weight: optional(int),
  )),
  posts: collection.with(schema: (
    title: str,
    description: optional(str),
    date: optional(datetime),
    updated: optional(datetime),
    draft: optional(bool),
    template: optional(str),
    tags: optional(array(str)),
  )),
  projects: collection.with(schema: (
    title: str,
    description: str,
    template: optional(str),
    languages: array(str),
    links: array(object((label: str, url: url))),
  )),
  legacy: collection.with(schema: (
    title: str,
    date: optional(datetime),
    template: optional(str),
    authors: optional(array(str)),
  )),
)
"#,
    )
    .unwrap();
    fs::write(
        project.join("templates/project.typ"),
        r#"#let render(site: (:), page: (:), pages: (), taxonomies: (:), body) = context {
  if target() == "html" {
    html.elem("html")[
      #html.elem("body")[
        #html.elem("h1")[#page.title]
        #html.elem("p", attrs: (id: "language"))[#page.languages.first()]
        #html.elem("a", attrs: (href: page.links.first().url))[#page.links.first().label]
        #html.elem("article")[#body]
      ]
    ]
  } else {
    body
  }
}
"#,
    )
    .unwrap();
    fs::write(
        project.join("content/projects/typshade.typ"),
        r#"#show: project.with(
  title: "typshade",
  description: "A Typst package for visualizing multiple-sequence alignments in bioinformatics.",
  template: "project.typ",
  languages: ("Typst",),
  links: (
    (label: "GitHub", url: "https://github.com/rice8y/typshade"),
  ),
)

Project body.
"#,
    )
    .unwrap();
    fs::write(
        project.join("content/legacy/toml.typ"),
        r#"---
title = "Legacy TOML"
date = "2026-06-23"
authors = ["Eito"]
---

Legacy body.
"#,
    )
    .unwrap();

    let build = Command::new(exe)
        .arg("build")
        .arg("--root")
        .arg(&project)
        .output()
        .expect("run typage build");
    assert!(
        build.status.success(),
        "build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    assert_exists(&project.join("public/index.html"));
    assert_exists(&project.join("public/projects/typshade/index.html"));
    assert_exists(&project.join("public/legacy/toml/index.html"));
    assert_exists(&project.join("public/.nojekyll"));
    assert_exists(&project.join("public/CNAME"));
    assert!(!project.join("public/config/index.html").exists());
    let project_html =
        fs::read_to_string(project.join("public/projects/typshade/index.html")).unwrap();
    assert!(project_html.contains("Typst"));
    assert!(project_html.contains("GitHub"));

    let doctor = Command::new(exe)
        .arg("doctor")
        .arg("--root")
        .arg(&project)
        .output()
        .expect("run typage doctor");
    assert!(
        doctor.status.success(),
        "doctor failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );

    let deploy_vercel = Command::new(exe)
        .arg("deploy")
        .arg("init")
        .arg("vercel")
        .arg("--root")
        .arg(&project)
        .output()
        .expect("run typage deploy init vercel");
    assert!(
        deploy_vercel.status.success(),
        "deploy init vercel failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&deploy_vercel.stdout),
        String::from_utf8_lossy(&deploy_vercel.stderr)
    );
    assert_exists(&project.join("vercel.json"));

    let deploy_doctor = Command::new(exe)
        .arg("deploy")
        .arg("doctor")
        .arg("--root")
        .arg(&project)
        .output()
        .expect("run typage deploy doctor");
    assert!(
        deploy_doctor.status.success(),
        "deploy doctor failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&deploy_doctor.stdout),
        String::from_utf8_lossy(&deploy_doctor.stderr)
    );

    let run_check = Command::new(exe)
        .arg("run")
        .arg("check")
        .arg("--root")
        .arg(&project)
        .output()
        .expect("run typage run check");
    assert!(
        run_check.status.success(),
        "run check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_check.stdout),
        String::from_utf8_lossy(&run_check.stderr)
    );

    let _ = fs::remove_dir_all(&project);
}
