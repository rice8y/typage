use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_project(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("typage-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

fn assert_exists(path: &Path) {
    assert!(path.exists(), "expected {} to exist", path.display());
}

#[test]
fn init_and_build_basic_project_with_embedded_typst() {
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
    let mut config = fs::read_to_string(project.join("config.toml")).unwrap();
    config.push_str(
        r#"

[[pdf_documents]]
path = "print.pdf"
title = "Print Version"
description = "Combined print document."
template = "print.typ"
sections = ["posts"]
sort_by = "date_desc"

[[pdf_documents]]
path = "projects.pdf"
title = "Projects"
template = "print.typ"
pages = ["projects/typshade.typ"]
"#,
    );
    fs::write(project.join("config.toml"), config).unwrap();

    let build = Command::new(exe)
        .env("PATH", "")
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
    assert_exists(&project.join("public/print.pdf"));
    assert_exists(&project.join("public/projects.pdf"));
    assert_exists(&project.join("public/.nojekyll"));
    assert_exists(&project.join("public/CNAME"));
    assert!(!project.join("public/config/index.html").exists());
    let project_html =
        fs::read_to_string(project.join("public/projects/typshade/index.html")).unwrap();
    assert!(project_html.contains("Typst"));
    assert!(project_html.contains("GitHub"));

    let cached_build = Command::new(exe)
        .env("PATH", "")
        .arg("build")
        .arg("--root")
        .arg(&project)
        .arg("--explain")
        .env("NO_COLOR", "1")
        .output()
        .expect("run cached typage build");
    assert!(
        cached_build.status.success(),
        "cached build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cached_build.stdout),
        String::from_utf8_lossy(&cached_build.stderr)
    );
    let cached_stdout = String::from_utf8_lossy(&cached_build.stdout);
    assert!(cached_stdout.contains("skip pdf document print.pdf"));

    let per_page_pdf = Command::new(exe)
        .env("PATH", "")
        .arg("build")
        .args(["--jobs", "1"])
        .arg("--root")
        .arg(&project)
        .arg("--pdf")
        .output()
        .expect("run typage build --pdf");
    assert!(
        per_page_pdf.status.success(),
        "build --pdf failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&per_page_pdf.stdout),
        String::from_utf8_lossy(&per_page_pdf.stderr)
    );
    assert_exists(&project.join("public/posts/hello/index.pdf"));
    assert_exists(&project.join("public/print.pdf"));

    let hello = project.join("content/posts/hello.typ");
    let body = fs::read_to_string(&hello).unwrap();
    fs::write(&hello, format!("{body}\nUpdated embedded build.\n")).unwrap();
    let rebuild = Command::new(exe)
        .env("PATH", "")
        .env("NO_COLOR", "1")
        .args(["build", "--explain"])
        .arg("--root")
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    assert!(
        fs::read_to_string(project.join("public/posts/hello/index.html"))
            .unwrap()
            .contains("Updated embedded build.")
    );
    assert!(!String::from_utf8_lossy(&rebuild.stdout).contains("skip pdf document print.pdf"));

    let bundle = Command::new(exe)
        .env("PATH", "")
        .arg("bundle")
        .arg("--root")
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        bundle.status.success(),
        "bundle failed\n{}",
        String::from_utf8_lossy(&bundle.stderr)
    );
    assert!(
        fs::read_to_string(project.join("public/posts/hello/index.html"))
            .unwrap()
            .contains("Updated embedded build.")
    );
    assert_exists(&project.join("public/style.css"));

    let doctor = Command::new(exe)
        .env("PATH", "")
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
