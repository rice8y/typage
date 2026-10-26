use super::outputs::{collect_reserved_output_paths, insert_output_path, pdf_document_public_path};
use super::*;
use crate::config::PdfDocumentConfig;
use std::path::Path;

pub(super) fn page_summary(title: &str, url: &str, section: &str, date: &str) -> PageSummary {
    PageSummary {
        kind: "page".to_string(),
        title: title.to_string(),
        url: url.to_string(),
        canonical_url: url.to_string(),
        slug: title.to_ascii_lowercase().replace(' ', "-"),
        path: url.trim_matches('/').to_string(),
        file_path: format!("{}.typ", url.trim_matches('/')),
        description: Some(format!("{title} feed")),
        date: Some(date.to_string()),
        updated: None,
        weight: None,
        section: section.to_string(),
        parent_section: None,
        ancestors: Vec::new(),
        tags: Vec::new(),
        categories: Vec::new(),
        aliases: Vec::new(),
        source: format!("{}.typ", url.trim_matches('/')),
        excerpt: None,
        toc: Vec::new(),
        fields: BTreeMap::new(),
    }
}

#[test]
fn resolves_internal_links_and_keeps_raw_spans() {
    let mut map = BTreeMap::new();
    map.insert("@/posts/hello.typ".to_string(), "/posts/hello/".to_string());
    let mut broken = Vec::new();
    let line = "#link(\"@/posts/hello.typ#sec\")[ok] and `@/posts/hello.typ`";
    let got = resolve_internal_links_line(line, &map, &mut broken);
    assert_eq!(
        got,
        "#link(\"/posts/hello/#sec\")[ok] and `@/posts/hello.typ`"
    );
    assert!(broken.is_empty());
}

#[test]
fn reports_broken_links() {
    let map = BTreeMap::new();
    let mut broken = Vec::new();
    let got = resolve_internal_links_line("#link(\"@/missing.typ\")[missing]", &map, &mut broken);
    assert_eq!(got, "#link(\"@/missing.typ\")[missing]");
    assert_eq!(broken, vec!["@/missing.typ".to_string()]);
}

#[test]
fn rejects_unsafe_aliases() {
    assert!(alias_to_url("old/hello").is_ok());
    assert!(alias_to_url("../escape").is_err());
    assert!(alias_to_url("").is_err());
}

#[test]
fn rejects_unsafe_pdf_document_paths() {
    let mut doc = PdfDocumentConfig {
        path: "print.pdf".to_string(),
        ..PdfDocumentConfig::default()
    };
    assert_eq!(pdf_document_public_path(&doc).unwrap(), "print.pdf");
    for path in ["", "/print.pdf", "../print.pdf", "print.html", "docs/"] {
        doc.path = path.to_string();
        assert!(pdf_document_public_path(&doc).is_err(), "{path}");
    }
}

#[test]
fn detects_pdf_document_static_output_collision() {
    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("typage-pdf-collision-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let static_root = tmp.join("static");
    let out_root = tmp.join("public");
    std::fs::create_dir_all(&static_root).unwrap();
    std::fs::write(static_root.join("print.pdf"), "static").unwrap();
    let mut outputs = collect_reserved_output_paths(
        &Config::default(),
        &[static_root],
        &out_root,
        &[],
        &[],
        false,
    )
    .unwrap();
    let doc = PdfDocumentConfig {
        path: "print.pdf".to_string(),
        ..PdfDocumentConfig::default()
    };
    let err = insert_output_path(
        &mut outputs,
        clean_path(&pdf_document_output_path(&out_root, &doc).unwrap()),
        "pdf document print.pdf".to_string(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("output collision"));
    let _ = std::fs::remove_dir_all(tmp);
}

#[test]
fn output_registry_rejects_file_directory_conflicts_in_both_orders() {
    for paths in [
        ["public/docs", "public/docs/index.html"],
        ["public/docs/index.html", "public/docs"],
    ] {
        let mut outputs = BTreeMap::new();
        insert_output_path(&mut outputs, paths[0].into(), "first".into()).unwrap();
        assert!(insert_output_path(&mut outputs, paths[1].into(), "second".into()).is_err());
        assert_eq!(outputs.len(), 1);
    }
}

#[test]
fn project_static_overrides_theme_static() {
    let tmp = tempfile::tempdir().unwrap();
    let theme = tmp.path().join("theme");
    let project = tmp.path().join("static");
    for dir in [&theme, &project] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("style.css"), "body {}").unwrap();
    }
    let outputs = collect_reserved_output_paths(
        &Config::default(),
        &[theme, project.clone()],
        &tmp.path().join("public"),
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(outputs.len(), 1);
    assert!(outputs
        .values()
        .next()
        .unwrap()
        .contains(&project.display().to_string()));
}

#[test]
fn root_level_pages_belong_to_pages_section() {
    assert_eq!(infer_section(Path::new("search.typ")), "pages");
    assert_eq!(infer_section(Path::new("posts/hello.typ")), "posts");
}

#[test]
fn underscore_typst_files_are_content_partials() {
    assert!(is_content_partial(Path::new("_prelude.typ")));
    assert!(is_content_partial(Path::new("components/_helpers.typ")));
    assert!(!is_content_partial(Path::new("_index.typ")));
    assert!(!is_content_partial(Path::new("index.typ")));
}

#[test]
fn rejects_dangerous_directory_policy() {
    let root = Path::new("/tmp/site");
    let cfg = Config {
        out_dir: PathBuf::from("public"),
        static_dir: PathBuf::from("public"),
        ..Config::default()
    };
    let errors = validate_directory_policy(
        root,
        &cfg,
        &root.join("content"),
        &root.join("templates"),
        &[root.join("public")],
        &root.join("public"),
    );
    assert!(!errors.is_empty());
}

#[test]
fn rejects_project_dirs_outside_root() {
    assert!(validate_project_relative_dir("content_dir", Path::new("../content")).is_some());
    assert!(validate_project_relative_dir("static_dir", Path::new("/tmp/static")).is_some());
    assert!(validate_project_relative_dir("out_dir", Path::new("public")).is_none());
}

#[test]
fn cache_decision_explains_miss() {
    let cache = BuildCache::default();
    let got = cache_decision(&cache, "page:x", "hash", &[], false);
    assert!(!got.hit);
    assert_eq!(got.reasons, vec!["no cache entry".to_string()]);
}

#[test]
fn current_input_args_spills_large_current_into_typage_package() {
    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("typage-current-input-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let cfg = Config::default();
    let large_current = format!(
        "{{\"title\":\"{}\"}}",
        "x".repeat(INLINE_TYPAGE_CURRENT_LIMIT)
    );

    let inputs =
        current_input_args(&tmp, &cfg, "generated:/huge-section/", &large_current).unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].0, "typage_current_file");
    assert!(inputs[0].1.starts_with("current/"));
    let current_path = typage_package_root(&tmp, &cfg).join(&inputs[0].1);
    assert_eq!(
        std::fs::read_to_string(current_path).unwrap(),
        large_current
    );

    let small_inputs = current_input_args(&tmp, &cfg, "page:/small/", "{}").unwrap();
    assert_eq!(
        small_inputs,
        vec![("typage_current".to_string(), "{}".to_string())]
    );
    let _ = std::fs::remove_dir_all(tmp);
}

#[test]
fn generated_current_json_flattens_summary_metadata_fields() {
    let cfg = Config::default();
    let mut item = page_summary("Symbol", "/api/lib/symbol/", "api/lib", "2026-01-01");
    item.fields.insert(
        "api_module".to_string(),
        MetadataField::from_toml(toml::Value::String("lib".to_string())),
    );
    let generated = GeneratedPage {
        kind: "section".to_string(),
        title: "lib".to_string(),
        description: None,
        url: "/api/lib/".to_string(),
        output_html: PathBuf::from("public/api/lib/index.html"),
        items: vec![item],
        page_number: 1,
        total_pages: 1,
        prev_title: None,
        prev_url: None,
        next_title: None,
        next_url: None,
    };

    let value: serde_json::Value =
        serde_json::from_str(&generated_current_json(&generated, &cfg).unwrap()).unwrap();
    let first = value["items"].as_array().unwrap().first().unwrap();
    assert!(first["api_module"]["__typage_native"].is_string());
    assert!(first.get("fields").is_none());
}

#[cfg(unix)]
#[test]
fn cache_decision_rebuilds_unsafe_output_symlink() {
    use std::os::unix::fs::symlink;

    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("typage-cache-link-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let outside = tmp.join("outside.html");
    let output = tmp.join("index.html");
    std::fs::write(&outside, "outside").unwrap();
    symlink(&outside, &output).unwrap();

    let mut cache = BuildCache::default();
    cache.entries.insert(
        "page:index.typ".to_string(),
        CacheEntry {
            hash: "hash".to_string(),
            outputs: vec![output.to_string_lossy().to_string()],
            dependencies: Some(Default::default()),
        },
    );
    let got = cache_decision(&cache, "page:index.typ", "hash", &[output], false);
    assert!(!got.hit);
    assert!(got
        .reasons
        .iter()
        .any(|reason| reason.contains("unsafe output path")));
    let _ = std::fs::remove_dir_all(tmp);
}

#[cfg(unix)]
#[test]
fn prepare_output_file_removes_dangling_output_symlink() {
    use std::os::unix::fs::symlink;

    let tmp = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "typage-prepare-dangling-link-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let public = tmp.join("public");
    let outside = tmp.join("outside");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let target = outside.join("missing.html");
    let output = public.join("index.html");
    symlink(&target, &output).unwrap();

    compiler::prepare_output_file(&output).unwrap();
    assert!(std::fs::symlink_metadata(&output).is_err());
    assert!(!target.exists());
    let _ = std::fs::remove_dir_all(tmp);
}

#[test]
fn effective_jobs_clamps_to_job_count() {
    assert_eq!(effective_jobs(Some(1), 0, 10), 1);
    assert_eq!(effective_jobs(Some(99), 0, 3), 3);
    assert_eq!(effective_jobs(Some(0), 0, 0), 0);
}

#[test]
fn writes_virtual_typage_package() {
    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("typage-pkg-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let cfg = Config::default();
    write_typage_package_data(&tmp, &cfg, &site_data_typ(&cfg, &[], &[], &BTreeMap::new()))
        .unwrap();
    let pkg = tmp.join(format!(".typage/packages/local/typage/{TYPAGE_VERSION}"));
    let lib = std::fs::read_to_string(pkg.join("lib.typ")).unwrap();
    assert!(lib.contains("sys.inputs.at"));
    assert!(lib.contains("typage_current"));
    assert!(lib.contains("typage_current_file"));
    assert!(lib.contains("#let asset"));
    assert!(lib.contains("#let section"));
    assert!(lib.contains("#let children"));
    assert!(lib.contains("#let ancestors"));
    assert!(lib.contains("#let siblings"));
    assert!(lib.contains("#let taxonomy-url"));
    assert!(!lib.contains("#let note"));
    assert!(!lib.contains("#let callout"));
    assert!(!lib.contains("#let card"));
    assert!(!lib.contains("#let fig"));
    assert!(!lib.contains("#let youtube"));
    assert!(!pkg.join("current.typ").exists());
    assert!(!tmp.join(".typage/packages/local/typssg/0.1.0").exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn writes_virtual_theme_package() {
    let tmp = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("typage-theme-pkg-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let cfg = Config::default();
    std::fs::create_dir_all(tmp.join("templates/components")).unwrap();
    std::fs::write(
        tmp.join("templates/components/lib.typ"),
        "#let note(body) = body\n",
    )
    .unwrap();
    write_theme_package_data(&tmp, &cfg).unwrap();
    let pkg = tmp.join(format!(
        ".typage/packages/local/typage-theme/{TYPAGE_VERSION}"
    ));
    let manifest = std::fs::read_to_string(pkg.join("typst.toml")).unwrap();
    let lib = std::fs::read_to_string(pkg.join("lib.typ")).unwrap();
    let meta = std::fs::read_to_string(pkg.join("metadata.typ")).unwrap();
    assert!(manifest.contains("name = \"typage-theme\""));
    assert!(lib.contains("#let note"));
    assert!(meta.contains("#let theme"));
    assert!(!tmp
        .join(".typage/packages/local/typssg-theme/0.1.0")
        .exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn expands_builtin_shortcodes() {
    let got = expand_shortcodes("before {{ note(text=\"hello\") }} after");
    assert!(got.contains("html.elem(\"aside\""));
    assert!(got.contains("hello"));
}

#[test]
fn package_imports_are_not_local_dependencies() {
    let deps = extract_dependency_paths(&format!(
        "#import \"@local/typage:{TYPAGE_VERSION}\": site\n#import \"@preview/cetz:0.5.2\""
    ));
    assert!(deps.is_empty());
    assert!(!is_local_typst_path(&format!(
        "@local/typage:{TYPAGE_VERSION}"
    )));
    assert!(!is_local_typst_path("@preview/cetz:0.5.2"));
    assert!(is_local_typst_path("./helpers.typ"));
}

#[test]
fn dependency_paths_ignore_raw_blocks_and_inline_code() {
    let deps = extract_dependency_paths(
        r#"#import "helpers.typ": x

````typ
```txt
inner fence
```
#let sample = read("example.dat", encoding: none)
````

Use `read("inline.dat", encoding: none)` in examples.
("icons", "loaded via read(..., encoding: none)"), ("next", "value")
"#,
    );
    assert_eq!(deps, BTreeSet::from(["helpers.typ".to_string()]));
}

#[test]
fn section_hierarchy_uses_nested_paths() {
    assert_eq!(
        infer_section(Path::new("posts/tutorials/intro.typ")),
        "posts/tutorials"
    );
    assert_eq!(
        parent_section_name("posts/tutorials").as_deref(),
        Some("posts")
    );
    assert_eq!(
        section_ancestors("posts/tutorials/deep"),
        vec!["posts".to_string(), "posts/tutorials".to_string()]
    );
    assert_eq!(section_url("posts/tutorials"), "/posts/tutorials/");
}

#[test]
fn toml_frontmatter_unknown_fields_become_metadata_fields() {
    let raw = r#"---
title = "Hello"
authors = ["Eito"]
---

Body
"#;
    let (fm, body) = split_metadata(Path::new("content/hello.typ"), raw).unwrap();
    assert_eq!(body, "Body\n");
    assert_eq!(fm.fields["authors"].export_json(), Some(json!(["Eito"])));
}

#[test]
fn toml_extra_table_is_rejected() {
    let raw = r#"---
title = "Hello"

[extra]
series = "examples"
---

Body
"#;
    let err = split_metadata(Path::new("content/hello.typ"), raw)
        .unwrap_err()
        .to_string();
    assert!(err.contains("`extra` is not supported"), "{err}");
}

#[test]
fn toml_frontmatter_still_splits_metadata() {
    let raw = r#"---
title = "Hello"
tags = ["typst", "ssg"]
---

Body
"#;
    let (meta, body) = split_metadata(Path::new("content/hello.typ"), raw).unwrap();
    assert_eq!(meta.title.as_deref(), Some("Hello"));
    assert_eq!(meta.tags, vec!["typst".to_string(), "ssg".to_string()]);
    assert_eq!(body, "Body\n");
}

#[test]
fn typst_project_metadata_directive_maps_custom_fields() {
    let raw = r#"#show: project.with(
  title: "typshade",
  description: "A Typst package for visualizing multiple-sequence alignments in bioinformatics.",
  date: "2026-05-23",
  updated: "2026-06-30",
  toc: false,
  languages: ("Typst",),
  links: (
    (label: "GitHub", url: "https://github.com/rice8y/typshade"),
    (label: "Typst Universe", url: "https://typst.app/universe/package/typshade/"),
  ),
)

Project body.
"#;
    let (meta, body) = split_metadata(Path::new("content/projects/typshade.typ"), raw).unwrap();
    assert_eq!(meta.title.as_deref(), Some("typshade"));
    assert_eq!(meta.date.as_deref(), Some("2026-05-23"));
    assert_eq!(meta.updated.as_deref(), Some("2026-06-30"));
    assert_eq!(meta.toc, Some(false));
    assert_eq!(body, "Project body.\n");
    assert_eq!(
        meta.fields["languages"].export_json(),
        Some(json!(["Typst"]))
    );
    assert_eq!(
        meta.fields["links"].export_json().unwrap()[0]["label"],
        "GitHub"
    );
}

#[test]
fn toml_frontmatter_and_typst_directive_conflict() {
    let raw = r#"---
title = "Hello"
---

#show: page.with(title: "Hello")

Body
"#;
    let err = split_metadata(Path::new("content/conflict.typ"), raw)
        .unwrap_err()
        .to_string();
    assert!(err.contains("cannot combine TOML front matter with Typst metadata directive"));
    assert!(err.contains("content/conflict.typ"));
}

#[test]
fn unsupported_metadata_expression_reports_path_and_position() {
    let raw = r#"#show: page.with(title: unknown_metadata_function("Hello"))

Body
"#;
    let err = split_metadata(Path::new("content/demo.typ"), raw)
        .unwrap_err()
        .to_string();
    assert!(err.contains("content/demo.typ:1:"), "{err}");
    assert!(err.contains("unknown variable"), "{err}");
    assert!(err.contains("unknown_metadata_function"), "{err}");
}

#[test]
fn site_data_contains_sections_tuple() {
    let cfg = Config::default();
    let mut page = PageSummary {
        kind: "page".to_string(),
        title: "Intro".to_string(),
        url: "/posts/tutorials/intro/".to_string(),
        canonical_url: "/posts/tutorials/intro/".to_string(),
        slug: "intro".to_string(),
        path: "posts/tutorials/intro".to_string(),
        file_path: "posts/tutorials/intro.typ".to_string(),
        description: None,
        date: Some("2026-06-23".to_string()),
        updated: Some("2026-06-24".to_string()),
        weight: Some(10),
        section: "posts/tutorials".to_string(),
        parent_section: Some("posts".to_string()),
        ancestors: vec!["posts".to_string()],
        tags: Vec::new(),
        categories: Vec::new(),
        aliases: Vec::new(),
        source: "posts/tutorials/intro.typ".to_string(),
        excerpt: None,
        toc: Vec::new(),
        fields: BTreeMap::new(),
    };
    page.fields.insert(
        "series".to_string(),
        MetadataField::from_toml(toml::Value::String("guide".to_string())),
    );
    let data = site_data_typ(&cfg, &[page], &[], &BTreeMap::new());
    assert!(data.contains("#let sections"));
    assert!(data.contains("posts/tutorials"));
    assert!(data.contains("parent:"));
    assert!(data.contains("ancestors:"));
}

#[test]
fn permalink_policy_expands_placeholders() {
    let cfg = Config {
        permalink: Some("/docs/:year/:month/:slug/".to_string()),
        ..Config::default()
    };
    let fm = FrontMatter {
        title: Some("Intro".to_string()),
        date: Some("2026-06-24".to_string()),
        slug: Some("getting-started".to_string()),
        ..FrontMatter::default()
    };
    let addr = page_address(&cfg, Path::new("posts/intro.typ"), "posts", &fm).unwrap();
    assert_eq!(addr.url, "/docs/2026/06/getting-started/");
    assert_eq!(addr.slug, "getting-started");
    assert_eq!(addr.path, "posts/intro");
    assert_eq!(addr.file_path, "posts/intro.typ");
}

#[test]
fn page_permalink_overrides_global_policy() {
    let cfg = Config {
        permalink: Some("/:section/:slug/".to_string()),
        ..Config::default()
    };
    let fm = FrontMatter {
        slug: Some("v0-23".to_string()),
        permalink: Some("/releases/:slug/".to_string()),
        ..FrontMatter::default()
    };
    let addr = page_address(&cfg, Path::new("posts/release.typ"), "posts", &fm).unwrap();
    assert_eq!(addr.url, "/releases/v0-23/");
}

#[test]
fn unsafe_slug_and_permalink_are_rejected() {
    let cfg = Config::default();
    let fm = FrontMatter {
        slug: Some("../escape".to_string()),
        ..FrontMatter::default()
    };
    assert!(page_address(&cfg, Path::new("posts/hello.typ"), "posts", &fm).is_err());
    let fm = FrontMatter {
        permalink: Some("/../escape/".to_string()),
        ..FrontMatter::default()
    };
    assert!(page_address(&cfg, Path::new("posts/hello.typ"), "posts", &fm).is_err());
}
