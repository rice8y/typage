use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use typst_syntax::ast::{self, AstNode};
use typst_syntax::{is_ident, LinkedNode};

use crate::config::{load_config, Config, FeedConfig, SearchConfig};
use crate::model::{
    BuildCache, BuildStats, CacheEntry, FrontMatter, GeneratedPage, MetadataField, Page,
    PageSummary, SkipStats, TocItem,
};
use crate::term;
use crate::util::{
    copy_dir, hash_strs, is_hidden, is_typst_file, normalize_path, relative_path,
    remove_dir_if_exists, slugify, to_posix_path, typst_array_str, typst_opt_string, typst_string,
    typst_tuple, write_if_changed,
};

const MIN_TYPST_VERSION: (u64, u64, u64) = (0, 15, 0);
const MIN_TYPST_VERSION_DISPLAY: &str = "0.15.0";
const TYPAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub root: PathBuf,
    pub drafts: bool,
    pub force: bool,
    pub typst_override: Option<String>,
    pub pdf: bool,
    pub keep_going: bool,
    pub jobs: Option<usize>,
    pub profile: bool,
    pub explain: bool,
    pub quiet: bool,
    pub verbose: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ThemeToml {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    min_typage: Option<String>,
    components: BTreeMap<String, bool>,
    extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone)]
struct ThemeReport {
    name: String,
    path: PathBuf,
    meta: Option<ThemeToml>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl ThemeReport {
    fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
struct ContentCollections {
    collections: BTreeMap<String, CollectionSchema>,
}

impl ContentCollections {
    fn schema_for(&self, section: &str) -> Option<&CollectionSchema> {
        self.collections.get(section).or_else(|| {
            section
                .split('/')
                .next()
                .and_then(|name| self.collections.get(name))
        })
    }
}

#[derive(Debug, Clone, Default)]
struct CollectionSchema {
    fields: BTreeMap<String, MetadataFieldSchema>,
}

#[derive(Debug, Clone)]
struct MetadataFieldSchema {
    optional: bool,
    kind: MetadataFieldKind,
}

#[derive(Debug, Clone)]
enum MetadataFieldKind {
    Builtin(String),
    Array(Box<MetadataFieldSchema>),
    Object(BTreeMap<String, MetadataFieldSchema>),
    Union(Vec<MetadataFieldSchema>),
    Any,
}

#[derive(Debug, Clone)]
struct CompileJob {
    label: String,
    input: PathBuf,
    html_output: PathBuf,
    pdf_output: Option<PathBuf>,
    html_features: String,
    current_json: String,
    cache_key: String,
    hash: String,
    outputs: Vec<PathBuf>,
    generated: bool,
}

#[derive(Debug, Clone)]
struct CompileReport {
    label: String,
    duration: Duration,
    output_count: usize,
    generated: bool,
}

pub fn init_project(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("content"))?;
    fs::create_dir_all(root.join("templates/components"))?;
    fs::create_dir_all(root.join("static"))?;
    write_if_missing(root.join("config.toml"), include_str!("../config.toml"))?;
    write_if_missing(
        root.join("templates/base.typ"),
        include_str!("../templates/base.typ"),
    )?;
    write_if_missing(
        root.join("templates/list.typ"),
        include_str!("../templates/list.typ"),
    )?;
    write_if_missing(
        root.join("templates/helpers.typ"),
        include_str!("../templates/helpers.typ"),
    )?;
    write_if_missing(
        root.join("templates/components/layout.typ"),
        include_str!("../templates/components/layout.typ"),
    )?;
    write_if_missing(
        root.join("templates/components/lib.typ"),
        include_str!("../templates/components/lib.typ"),
    )?;
    write_if_missing(
        root.join("templates/components/callout.typ"),
        include_str!("../templates/components/callout.typ"),
    )?;
    write_if_missing(
        root.join("templates/components/card.typ"),
        include_str!("../templates/components/card.typ"),
    )?;
    write_if_missing(
        root.join("templates/components/media.typ"),
        include_str!("../templates/components/media.typ"),
    )?;
    write_if_missing(
        root.join("content/index.typ"),
        include_str!("../content/index.typ"),
    )?;
    write_if_missing(
        root.join("content/config.typ"),
        include_str!("../content/config.typ"),
    )?;
    write_if_missing(
        root.join("content/about.typ"),
        include_str!("../content/about.typ"),
    )?;
    write_if_missing(
        root.join("content/posts/_index.typ"),
        include_str!("../content/posts/_index.typ"),
    )?;
    write_if_missing(
        root.join("content/posts/hello.typ"),
        include_str!("../content/posts/hello.typ"),
    )?;
    write_if_missing(
        root.join("static/style.css"),
        include_str!("../static/style.css"),
    )?;
    println!("initialized {}", root.display());
    Ok(())
}

pub fn new_theme(root: PathBuf, name: String) -> Result<()> {
    let root = normalize_path(&root)?;
    validate_theme_name(&name)?;
    let theme_root = root.join("themes").join(&name);
    fs::create_dir_all(theme_root.join("templates/components"))?;
    fs::create_dir_all(theme_root.join("static"))?;
    write_if_missing(theme_root.join("theme.toml"), &default_theme_toml(&name))?;
    write_if_missing(
        theme_root.join("templates/base.typ"),
        include_str!("../templates/base.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/list.typ"),
        include_str!("../templates/list.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/helpers.typ"),
        include_str!("../templates/helpers.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/layout.typ"),
        include_str!("../templates/components/layout.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/lib.typ"),
        include_str!("../templates/components/lib.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/callout.typ"),
        include_str!("../templates/components/callout.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/card.typ"),
        include_str!("../templates/components/card.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/media.typ"),
        include_str!("../templates/components/media.typ"),
    )?;
    write_if_missing(
        theme_root.join("static/style.css"),
        include_str!("../static/style.css"),
    )?;
    println!("created theme {}", theme_root.display());
    println!(
        "enable it with: theme = {} in config.toml",
        toml_string(&name)
    );
    Ok(())
}

pub fn list_themes(root: PathBuf, verbose: bool) -> Result<()> {
    let root = normalize_path(&root)?;
    let themes_root = root.join("themes");
    if !themes_root.exists() {
        println!("no themes directory: {}", themes_root.display());
        return Ok(());
    }
    println!("themes:");
    for entry in fs::read_dir(&themes_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let report = inspect_theme(&entry.path(), Some(&name));
        if let Some(meta) = &report.meta {
            let version = meta.version.as_deref().unwrap_or("");
            let description = meta.description.as_deref().unwrap_or("");
            let status = if report.ok() {
                term::green("ok")
            } else {
                term::red("issues")
            };
            println!("  {name:16} {version:8} {status:8} {description}");
            if verbose {
                println!("    path {}", report.path.display());
                if let Some(min) = &meta.min_typage {
                    println!("    min_typage {min}");
                }
                let comps = enabled_components(meta);
                println!(
                    "    components {}",
                    if comps.is_empty() {
                        "-".to_string()
                    } else {
                        comps.join(", ")
                    }
                );
                let extras = meta.extra.keys().cloned().collect::<Vec<_>>();
                println!(
                    "    extra {}",
                    if extras.is_empty() {
                        "-".to_string()
                    } else {
                        extras.join(", ")
                    }
                );
                for warning in &report.warnings {
                    println!("    {} {warning}", term::yellow("warning"));
                }
                for error in &report.errors {
                    println!("    {} {error}", term::red("error"));
                }
            }
        } else {
            println!("  {name:16} {} missing theme.toml", term::red("issues"));
            if verbose {
                for error in &report.errors {
                    println!("    {} {error}", term::red("error"));
                }
            }
        }
    }
    Ok(())
}

pub fn theme_info(root: PathBuf, name: Option<String>) -> Result<()> {
    let root = normalize_path(&root)?;
    let (name, theme_root) = resolve_theme_target(&root, name)?;
    let report = inspect_theme(&theme_root, Some(&name));
    print_theme_report(&report, true);
    if report.ok() {
        Ok(())
    } else {
        bail!("theme {} has {} issue(s)", name, report.errors.len())
    }
}

pub fn theme_check(root: PathBuf, name: Option<String>) -> Result<()> {
    let root = normalize_path(&root)?;
    let (name, theme_root) = resolve_theme_target(&root, name)?;
    let report = inspect_theme(&theme_root, Some(&name));
    print_theme_report(&report, false);
    if report.ok() {
        println!("theme check: ok");
        Ok(())
    } else {
        bail!(
            "theme {} has {} issue(s)\n\n{}",
            name,
            report.errors.len(),
            report.errors.join("\n")
        )
    }
}

fn resolve_theme_target(root: &Path, name: Option<String>) -> Result<(String, PathBuf)> {
    if let Some(name) = name {
        validate_theme_name(&name)?;
        return Ok((name.clone(), root.join("themes").join(name)));
    }
    let cfg = load_config(root)?;
    if let Some(name) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        return Ok((name.to_string(), root.join("themes").join(name)));
    }
    bail!("no theme specified. Pass a theme name or set `theme = ...` in config.toml")
}

fn default_theme_toml(name: &str) -> String {
    format!(
        r##"name = {}
version = "0.1.0"
description = "A typage theme."
min_typage = "{}"

[components]
note = true
callout = true
card = true
fig = true
youtube = true
page_link = true
taxonomy_link = true

[extra]
accent = "#2563eb"
"##,
        toml_string(name),
        TYPAGE_VERSION
    )
}

fn read_theme_toml(theme_root: &Path) -> Result<ThemeToml> {
    let raw = fs::read_to_string(theme_root.join("theme.toml"))
        .with_context(|| format!("failed to read {}", theme_root.join("theme.toml").display()))?;
    let mut meta: ThemeToml = toml::from_str(&raw).with_context(|| {
        format!(
            "failed to parse {}",
            theme_root.join("theme.toml").display()
        )
    })?;
    if meta.name.as_deref().unwrap_or("").trim().is_empty() {
        meta.name = theme_root
            .file_name()
            .map(|s| s.to_string_lossy().to_string());
    }
    Ok(meta)
}

fn inspect_theme(theme_root: &Path, expected_name: Option<&str>) -> ThemeReport {
    let mut report = ThemeReport {
        name: expected_name
            .unwrap_or_else(|| {
                theme_root
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("theme")
            })
            .to_string(),
        path: theme_root.to_path_buf(),
        meta: None,
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    if !theme_root.exists() {
        report.errors.push(format!(
            "theme directory is missing: {}",
            theme_root.display()
        ));
        return report;
    }
    let meta = match read_theme_toml(theme_root) {
        Ok(meta) => meta,
        Err(err) => {
            report
                .errors
                .push(format!("theme metadata is missing or invalid: {err:?}"));
            return report;
        }
    };
    if let Some(expected) = expected_name {
        if meta.name.as_deref() != Some(expected) {
            report.warnings.push(format!(
                "theme.toml name {:?} differs from directory name {:?}",
                meta.name, expected
            ));
        }
    }
    if meta.version.as_deref().unwrap_or("").trim().is_empty() {
        report
            .errors
            .push("theme.toml is missing version".to_string());
    }
    if let Some(min) = &meta.min_typage {
        if version_lt(env!("CARGO_PKG_VERSION"), min) {
            report.errors.push(format!(
                "theme requires typage >= {min}, current is {}",
                env!("CARGO_PKG_VERSION")
            ));
        }
    } else {
        report
            .warnings
            .push("theme.toml does not specify min_typage".to_string());
    }
    let templates = theme_root.join("templates");
    let components = templates.join("components");
    let entry = components.join("lib.typ");
    if !templates.exists() {
        report.errors.push(format!(
            "theme templates dir is missing: {}",
            templates.display()
        ));
    }
    if !theme_root.join("static").exists() {
        report.warnings.push(format!(
            "theme static dir is missing: {}",
            theme_root.join("static").display()
        ));
    }
    if !components.exists() {
        report.errors.push(format!(
            "theme components dir is missing: {}",
            components.display()
        ));
    } else if !entry.exists() {
        report.errors.push(format!(
            "theme components entrypoint is missing: {}",
            entry.display()
        ));
    } else {
        match fs::read_to_string(&entry) {
            Ok(lib) => {
                for component in enabled_components(&meta) {
                    let export_name = component_name_to_export(&component);
                    if !component_exported(&lib, &export_name) {
                        report.errors.push(format!("theme declares component `{component}` but templates/components/lib.typ does not appear to export `{export_name}`"));
                    }
                }
            }
            Err(err) => report
                .errors
                .push(format!("failed to read {}: {err}", entry.display())),
        }
    }
    report.name = meta.name.clone().unwrap_or(report.name.clone());
    report.meta = Some(meta);
    report
}

fn enabled_components(meta: &ThemeToml) -> Vec<String> {
    meta.components
        .iter()
        .filter_map(|(k, v)| if *v { Some(k.clone()) } else { None })
        .collect()
}

fn component_name_to_export(name: &str) -> String {
    name.replace('-', "_").replace("_link", "-link")
}

fn component_exported(lib: &str, name: &str) -> bool {
    lib.contains(&format!("#let {name}"))
        || lib.contains(&format!(": {name}"))
        || lib.contains(&format!(", {name}"))
        || lib.contains(&format!(" {name}"))
}

fn print_theme_report(report: &ThemeReport, verbose: bool) {
    println!("theme: {}", report.name);
    println!("path: {}", report.path.display());
    if let Some(meta) = &report.meta {
        println!("version: {}", meta.version.as_deref().unwrap_or(""));
        if let Some(desc) = &meta.description {
            println!("description: {desc}");
        }
        if let Some(min) = &meta.min_typage {
            println!("min_typage: {min}");
        }
        let components = enabled_components(meta);
        println!(
            "components: {}",
            if components.is_empty() {
                "-".to_string()
            } else {
                components.join(", ")
            }
        );
        if verbose {
            if !meta.extra.is_empty() {
                println!("extra:");
                for (key, value) in &meta.extra {
                    println!("  {key}: {:?}", value);
                }
            }
        }
    }
    for warning in &report.warnings {
        println!("{} {warning}", term::yellow("warning"));
    }
    for error in &report.errors {
        println!("{} {error}", term::red("error"));
    }
}

fn version_lt(current: &str, min: &str) -> bool {
    parse_version_tuple(current) < parse_version_tuple(min)
}

fn parse_version_tuple(input: &str) -> (u64, u64, u64) {
    let mut parts = input
        .split(|c: char| !(c.is_ascii_digit()))
        .filter(|s| !s.is_empty());
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn theme_metadata_typ(meta: &ThemeToml) -> String {
    let components = meta
        .components
        .iter()
        .map(|(k, v)| format!("{}: {}", typst_ident(k), v))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "#let theme = (name: {}, version: {}, description: {}, min_typage: {}, components: ({}), extra: {})\n",
        typst_opt_string(&meta.name),
        typst_opt_string(&meta.version),
        typst_opt_string(&meta.description),
        typst_opt_string(&meta.min_typage),
        components,
        typst_toml_table(&meta.extra),
    )
}

fn validate_theme_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("theme name must not be empty");
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        bail!("invalid theme name: {name}");
    }
    Ok(())
}

fn templates_root(root: &Path, cfg: &Config) -> PathBuf {
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        let themed = root.join("themes").join(theme).join("templates");
        if themed.exists() {
            return themed;
        }
    }
    root.join(&cfg.templates_dir)
}

fn static_roots(root: &Path, cfg: &Config) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        roots.push(root.join("themes").join(theme).join("static"));
    }
    roots.push(root.join(&cfg.static_dir));
    roots
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn validate_directory_policy(
    root: &Path,
    cfg: &Config,
    content_root: &Path,
    templates_root: &Path,
    static_roots: &[PathBuf],
    out_root: &Path,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, path) in [
        ("content_dir", &cfg.content_dir),
        ("templates_dir", &cfg.templates_dir),
        ("static_dir", &cfg.static_dir),
        ("out_dir", &cfg.out_dir),
        ("cache_dir", &cfg.cache_dir),
    ] {
        if let Some(error) = validate_project_relative_dir(name, path) {
            errors.push(error);
        }
    }
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Err(err) = validate_theme_name(theme) {
            errors.push(err.to_string());
        }
    }
    let root = clean_path(root);
    let out = clean_path(out_root);
    let cache = clean_path(&root.join(&cfg.cache_dir));
    let protected = [
        ("content_dir", clean_path(content_root)),
        ("templates_dir", clean_path(templates_root)),
        ("cache_dir", cache),
    ];
    if out == root {
        errors.push("out_dir must not be the project root".to_string());
    }
    for (name, path) in protected {
        if out == path {
            errors.push(format!("out_dir must not equal {name}: {}", path.display()));
        } else if out.starts_with(&path) {
            errors.push(format!(
                "out_dir must not be inside {name}: {}",
                path.display()
            ));
        } else if path.starts_with(&out) {
            errors.push(format!(
                "out_dir must not contain {name}: {}",
                path.display()
            ));
        }
    }
    for static_root in static_roots {
        let static_root = clean_path(static_root);
        if out == static_root {
            errors.push(format!(
                "out_dir must not equal static_dir/theme static dir: {}",
                static_root.display()
            ));
        } else if out.starts_with(&static_root) {
            errors.push(format!(
                "out_dir must not be inside static asset input dir: {}",
                static_root.display()
            ));
        } else if static_root.starts_with(&out) {
            errors.push(format!(
                "out_dir must not contain static asset input dir: {}",
                static_root.display()
            ));
        }
    }
    errors
}

fn validate_project_relative_dir(name: &str, path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return Some(format!("{name} must not be empty"));
    }
    if path.is_absolute() {
        return Some(format!(
            "{name} must be relative to the project root: {}",
            path.display()
        ));
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => {
                return Some(format!(
                    "{name} must not contain root, prefix, or parent components: {}",
                    path.display()
                ));
            }
        }
    }
    None
}

pub fn new_page(
    root: PathBuf,
    rel_path: PathBuf,
    title: Option<String>,
    date: Option<String>,
    draft: bool,
) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    let content_root = root.join(&cfg.content_dir);
    let mut rel = rel_path;
    if rel.extension().is_none() {
        rel.set_extension("typ");
    }
    validate_relative_content_path(&rel)?;
    let path = content_root.join(&rel);
    if path.exists() {
        bail!("refusing to overwrite existing page: {}", path.display());
    }
    let title = title.unwrap_or_else(|| default_title(&path));
    let mut fm = String::new();
    fm.push_str("#show: page.with(\n");
    fm.push_str(&format!("  title: {},\n", typst_string(&title)));
    if let Some(date) = date {
        fm.push_str(&format!("  date: {},\n", typst_string(&date)));
    }
    if draft {
        fm.push_str("  draft: true,\n");
    }
    fm.push_str("  tags: (),\n");
    fm.push_str(")\n\nWrite your content here.\n");
    write_if_changed(&path, &fm)?;
    println!("created {}", path.display());
    Ok(())
}

pub fn doctor(root: PathBuf, typst_override: Option<String>, drafts: bool) -> Result<()> {
    let root = normalize_path(&root)?;
    let mut cfg = load_config(&root)?;
    if let Some(typst) = typst_override {
        cfg.typst = typst;
    }
    let content_root = root.join(&cfg.content_dir);
    let templates_root = templates_root(&root, &cfg);
    let static_roots = static_roots(&root, &cfg);
    let out_root = root.join(&cfg.out_dir);
    let mut errors = Vec::<String>::new();
    errors.extend(validate_directory_policy(
        &root,
        &cfg,
        &content_root,
        &templates_root,
        &static_roots,
        &out_root,
    ));

    println!("root: {}", root.display());
    println!("config: {}", root.join("config.toml").display());
    match check_typst_requirement(&cfg.typst) {
        Ok(version) => {
            println!("typst: ok ({version}; requires {MIN_TYPST_VERSION_DISPLAY} or later)")
        }
        Err(err) => errors.push(err.to_string()),
    }
    for (name, path) in [("content", &content_root), ("templates", &templates_root)] {
        if path.exists() {
            println!("{name}: ok ({})", path.display());
        } else {
            errors.push(format!("missing {name} dir: {}", path.display()));
        }
    }
    let components_entry = templates_root.join("components/lib.typ");
    if components_entry.exists() {
        println!("components: ok ({})", components_entry.display());
    } else {
        println!("components: missing ({})", components_entry.display());
    }
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        let theme_root = root.join("themes").join(theme);
        let report = inspect_theme(&theme_root, Some(theme));
        println!("theme: {theme} ({})", theme_root.display());
        if let Some(meta) = &report.meta {
            println!(
                "theme metadata: version={} description={}",
                meta.version.as_deref().unwrap_or(""),
                meta.description.as_deref().unwrap_or("")
            );
            if let Some(min) = &meta.min_typage {
                println!("theme min_typage: {min}");
            }
            let components = enabled_components(meta);
            println!(
                "theme components: {}",
                if components.is_empty() {
                    "-".to_string()
                } else {
                    components.join(", ")
                }
            );
        }
        for warning in report.warnings {
            println!("{} {warning}", term::yellow("warning"));
        }
        errors.extend(report.errors);
    }
    for path in &static_roots {
        if path.exists() {
            println!("static: ok ({})", path.display());
        } else {
            println!("static: missing ({})", path.display());
        }
    }
    println!("out: {}", out_root.display());
    println!("cache: {}", root.join(&cfg.cache_dir).display());
    if cfg.base_url.trim().is_empty() && cfg.sitemap {
        errors.push("base_url is empty but sitemap is enabled".to_string());
    }

    let collections = load_content_collections(&content_root)?;
    let section_meta = discover_sections(&content_root)?;
    let (mut pages, skipped) =
        discover_pages(&root, &cfg, &content_root, &out_root, drafts, &collections)?;
    assign_prev_next(&mut pages, &section_meta);
    let link_map = build_link_map(&pages);
    for page in &mut pages {
        let (toc, body, broken_links) =
            preprocess_body(&page.body, &link_map, page.meta.toc.unwrap_or(true));
        if !broken_links.is_empty() {
            errors.push(format!(
                "broken internal link(s) in {}: {}",
                page.source.display(),
                broken_links.join(", ")
            ));
        }
        page.toc = toc;
        page.processed_body = body;
        if let (Some(date), Some(updated)) = (
            simple_date_prefix(page.meta.date.as_deref()),
            simple_date_prefix(page.meta.updated.as_deref()),
        ) {
            if updated < date {
                println!(
                    "{} updated is before date in {}: updated={} date={}",
                    term::yellow("warning"),
                    page.source.display(),
                    updated,
                    date
                );
            }
        }
        if let Err(err) = template_hash(&templates_root, &page.template) {
            errors.push(format!(
                "template error for {}: {err:?}",
                page.source.display()
            ));
        }
        for warning in dependency_diagnostics(&page.source, &page.body) {
            errors.push(warning);
        }
    }
    let summaries = pages.iter().map(Page::summary).collect::<Vec<_>>();
    let generated = make_generated_pages(&cfg, &out_root, &summaries, &section_meta);
    if let Err(err) = validate_routes(&pages, &generated) {
        errors.push(format!("route validation failed: {err:?}"));
    }
    if let Err(err) = template_hash(&templates_root, &cfg.list_template) {
        errors.push(format!("list template error: {err:?}"));
    }
    println!("pages: {}", pages.len());
    println!("generated: {}", generated.len());
    println!(
        "skipped: drafts={} future={} expired={} total={}",
        skipped.drafts,
        skipped.future,
        skipped.expired,
        skipped.total()
    );
    for warning in search_diagnostics(&cfg.search, &pages) {
        println!("{} {warning}", term::yellow("warning"));
    }
    if errors.is_empty() {
        println!("doctor: ok");
        Ok(())
    } else {
        bail!(
            "doctor found {} issue(s)\n\n{}",
            errors.len(),
            errors.join("\n")
        );
    }
}

fn check_typst_requirement(typst: &str) -> Result<String> {
    let out = Command::new(typst)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run typst command {typst:?}"))?;
    if !out.status.success() {
        bail!(
            "typst command exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let reported = if stdout.is_empty() { &stderr } else { &stdout };
    let version = parse_typst_version(reported).ok_or_else(|| {
        anyhow::anyhow!("failed to parse typst version from output: {reported:?}")
    })?;
    if version < MIN_TYPST_VERSION {
        bail!(
            "Typst {MIN_TYPST_VERSION_DISPLAY} or later is required; found {reported}. Typage uses Typst HTML export and bundle export features."
        );
    }
    Ok(reported.to_string())
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

fn validate_relative_content_path(path: &Path) -> Result<()> {
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => bail!("invalid content path: {}", path.display()),
        }
    }
    if path.to_string_lossy().contains('\\') {
        bail!("invalid content path: {}", path.display());
    }
    Ok(())
}

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn write_if_missing(path: PathBuf, contents: &str) -> Result<()> {
    if path.exists() {
        println!("skip existing {}", path.display());
        return Ok(());
    }
    write_if_changed(&path, contents)?;
    println!("write {}", path.display());
    Ok(())
}

pub fn clean(root: PathBuf) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    remove_dir_if_exists(&root.join(&cfg.out_dir))?;
    remove_dir_if_exists(&root.join(&cfg.cache_dir))?;
    remove_page_wrappers(&root.join(&cfg.content_dir))?;
    println!("cleaned {}", root.display());
    Ok(())
}

fn remove_page_wrappers(content_root: &Path) -> Result<()> {
    if !content_root.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(content_root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy();
            if name.starts_with(".typage.") && name.ends_with(".typ") {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

pub fn build_site(opts: &BuildOptions) -> Result<BuildStats> {
    let root = normalize_path(&opts.root)?;
    let mut cfg = load_config(&root)?;
    if let Some(typst) = &opts.typst_override {
        cfg.typst = typst.clone();
    }
    check_typst_requirement(&cfg.typst)?;
    let started = Instant::now();
    let cfg_hash_str = cfg_hash(&cfg);
    let mut template_hashes = BTreeMap::<String, String>::new();

    let content_root = root.join(&cfg.content_dir);
    let templates_root = templates_root(&root, &cfg);
    let static_roots = static_roots(&root, &cfg);
    let out_root = root.join(&cfg.out_dir);
    let policy_errors = validate_directory_policy(
        &root,
        &cfg,
        &content_root,
        &templates_root,
        &static_roots,
        &out_root,
    );
    if !policy_errors.is_empty() {
        bail!(
            "invalid directory configuration\n\n{}",
            policy_errors.join("\n")
        );
    }
    let cache_root = root.join(&cfg.cache_dir);
    let wrappers_root = cache_root.join("wrappers");
    let generated_root = cache_root.join("generated");
    let data_path = cache_root.join("site.typ");
    let cache_path = cache_root.join("build-cache.json");

    fs::create_dir_all(&out_root)?;
    fs::create_dir_all(&wrappers_root)?;
    fs::create_dir_all(&generated_root)?;

    let mut cache = read_cache(&cache_path)?;
    let collections = load_content_collections(&content_root)?;
    let section_meta = discover_sections(&content_root)?;
    let (mut pages, skipped) = discover_pages(
        &root,
        &cfg,
        &content_root,
        &out_root,
        opts.drafts,
        &collections,
    )?;
    assign_prev_next(&mut pages, &section_meta);
    let link_map = build_link_map(&pages);
    for page in &mut pages {
        let (toc, body, broken_links) =
            preprocess_body(&page.body, &link_map, page.meta.toc.unwrap_or(true));
        if !broken_links.is_empty() {
            let msg = format!(
                "broken internal link(s) in {}: {}",
                page.source.display(),
                broken_links.join(", ")
            );
            if cfg.fail_on_broken_links {
                bail!(msg);
            } else {
                eprintln!("warning: {msg}");
            }
        }
        page.toc = toc;
        page.processed_body = body;
    }

    let summaries = pages.iter().map(Page::summary).collect::<Vec<_>>();
    let generated = make_generated_pages(&cfg, &out_root, &summaries, &section_meta);
    validate_routes(&pages, &generated)?;
    let site_data = site_data_typ(&cfg, &summaries, &generated, &section_meta);
    let site_graph_hash = hash_strs(&[&site_data]);
    write_if_changed(&data_path, &site_data)?;
    write_typage_package_data(&root, &cfg, &site_data)?;
    write_theme_package_data(&root, &cfg)?;
    for page in &mut pages {
        let meta_hash = serde_json::to_string(&page.meta)?;
        let dep_hash = dependency_hash(&page.source, &page.body)?;
        if opts.explain && !opts.quiet {
            for warning in dependency_diagnostics(&page.source, &page.body) {
                eprintln!("  {} {}", term::yellow("warning"), warning);
            }
        }
        page.hash = hash_strs(&[
            &page.processed_body,
            &meta_hash,
            &dep_hash,
            &site_graph_hash,
            &cfg_hash_str,
            &cached_template_hash(&mut template_hashes, &templates_root, &page.template)?,
        ]);
    }

    let mut stats = BuildStats::default();
    let mut failures = Vec::<String>::new();
    let mut compile_jobs = Vec::<CompileJob>::new();
    stats.drafts = skipped.drafts;
    stats.future = skipped.future;
    stats.expired = skipped.expired;

    for page in &pages {
        let should_pdf = opts.pdf || cfg.build_pdf || page.meta.build_pdf.unwrap_or(false);
        let outputs = page_outputs(page, should_pdf);
        let cache_key = format!("page:{}", page.rel.to_string_lossy().replace('\\', "/"));
        let decision = cache_decision(&cache, &cache_key, &page.hash, &outputs, opts.force);
        if decision.hit {
            if opts.explain && !opts.quiet {
                print_explain(
                    "skip",
                    &page.source.display().to_string(),
                    &decision.reasons,
                );
            }
            cache.entries.insert(
                cache_key.clone(),
                CacheEntry {
                    hash: page.hash.clone(),
                    outputs: outputs_to_strings(&outputs),
                },
            );
            stats.skipped += 1;
            continue;
        } else if opts.explain && !opts.quiet {
            print_explain(
                "rebuild",
                &page.source.display().to_string(),
                &decision.reasons,
            );
        }
        let wrapper_path = page_wrapper_path(&wrappers_root, page);
        let wrapper =
            page_wrapper_source(&root, &cfg, &data_path, page, &wrapper_path, &collections)?;
        write_if_changed(&wrapper_path, &wrapper)?;
        compile_jobs.push(CompileJob {
            label: page.source.display().to_string(),
            input: wrapper_path,
            html_output: page.output_html.clone(),
            pdf_output: should_pdf.then(|| page.output_pdf.clone()),
            html_features: cfg.features.clone(),
            current_json: page_current_json(page)?,
            cache_key,
            hash: page.hash.clone(),
            outputs,
            generated: false,
        });
    }

    for gen in &generated {
        let hash = hash_strs(&[
            &serde_json::to_string(&gen.items)?,
            &gen.title,
            &site_graph_hash,
            &cfg_hash_str,
            &cached_template_hash(&mut template_hashes, &templates_root, &cfg.list_template)?,
        ]);
        let outputs = vec![gen.output_html.clone()];
        let cache_key = format!("generated:{}", gen.url);
        let decision = cache_decision(&cache, &cache_key, &hash, &outputs, opts.force);
        if decision.hit {
            if opts.explain && !opts.quiet {
                print_explain("skip", &gen.url, &decision.reasons);
            }
            cache.entries.insert(
                cache_key.clone(),
                CacheEntry {
                    hash: hash.clone(),
                    outputs: outputs_to_strings(&outputs),
                },
            );
            stats.skipped += 1;
            continue;
        } else if opts.explain && !opts.quiet {
            print_explain("rebuild", &gen.url, &decision.reasons);
        }
        let wrapper = generated_wrapper_source(&root, &cfg, &data_path, gen)?;
        let wrapper_path = generated_root.join(format!("{}.typ", slugify(&gen.url)));
        write_if_changed(&wrapper_path, &wrapper)?;
        compile_jobs.push(CompileJob {
            label: gen.url.clone(),
            input: wrapper_path,
            html_output: gen.output_html.clone(),
            pdf_output: None,
            html_features: cfg.features.clone(),
            current_json: generated_current_json(gen, &cfg)?,
            cache_key,
            hash,
            outputs,
            generated: true,
        });
    }

    let compile_concurrency = effective_jobs(opts.jobs, cfg.jobs, compile_jobs.len());
    if !opts.quiet {
        println!(
            "{} {} {}",
            term::bold("typage"),
            term::cyan("build"),
            term::dim(format!(
                "({} compile job{})",
                compile_concurrency,
                if compile_concurrency == 1 { "" } else { "s" }
            ))
        );
    }
    let compile_started = Instant::now();
    let outcomes = run_compile_jobs(&cfg, &root, compile_jobs, compile_concurrency);
    let compile_wall = compile_started.elapsed();
    let mut reports = Vec::<CompileReport>::new();
    for (job, result) in outcomes {
        match result {
            Ok(report) => {
                if !opts.quiet && opts.verbose {
                    let kind = if report.generated {
                        "generated"
                    } else {
                        "page"
                    };
                    println!(
                        "  {} {} {} {}",
                        term::green("✓"),
                        term::dim(kind),
                        report.label,
                        term::dim(format_duration(report.duration))
                    );
                }
                cache.entries.insert(
                    job.cache_key.clone(),
                    CacheEntry {
                        hash: job.hash.clone(),
                        outputs: outputs_to_strings(&job.outputs),
                    },
                );
                if job.generated {
                    stats.generated += 1;
                } else {
                    stats.built += 1;
                }
                reports.push(report);
            }
            Err(err) => {
                stats.failed += 1;
                if opts.keep_going {
                    eprintln!("  {} {}", term::red("✗"), job.label);
                    failures.push(format!("{}\n{err:?}", job.label));
                } else {
                    return Err(err);
                }
            }
        }
    }

    for static_root in &static_roots {
        copy_dir(static_root, &out_root)?;
    }
    write_alias_pages(&out_root, &pages)?;
    if cfg.sitemap {
        write_sitemap(&cfg, &out_root, &summaries, &generated)?;
    }
    if cfg.feed {
        write_configured_feeds(&cfg, &out_root, &summaries)?;
        if cfg.atom_feed {
            write_atom_feed(&cfg, &out_root, &summaries)?;
        }
    }
    if cfg.robots {
        write_robots(&cfg, &out_root)?;
    }
    if cfg.search.enabled {
        write_search_index(&out_root, &pages, &cfg.search)?;
    }
    let desired_outputs =
        desired_public_outputs(&cfg, &static_roots, &out_root, &pages, &generated, opts.pdf)?;
    cleanup_public_outputs(&out_root, &desired_outputs)?;
    write_cache(&cache_path, &cache)?;

    if !opts.quiet {
        print_build_summary(&stats, compile_concurrency, &out_root, started.elapsed());
        if opts.profile {
            print_profile(&reports, compile_wall);
        }
    }
    if !failures.is_empty() {
        bail!(
            "build completed with {} failure(s)\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
    Ok(stats)
}

pub fn bundle_site(root: PathBuf, drafts: bool, typst_override: Option<String>) -> Result<()> {
    let root = normalize_path(&root)?;
    let mut cfg = load_config(&root)?;
    if let Some(typst) = typst_override {
        cfg.typst = typst;
    }
    check_typst_requirement(&cfg.typst)?;
    let content_root = root.join(&cfg.content_dir);
    let out_root = root.join(&cfg.out_dir);
    let cache_root = root.join(&cfg.cache_dir);
    fs::create_dir_all(&out_root)?;
    fs::create_dir_all(&cache_root)?;

    let collections = load_content_collections(&content_root)?;
    let section_meta = discover_sections(&content_root)?;
    let (mut pages, skipped) =
        discover_pages(&root, &cfg, &content_root, &out_root, drafts, &collections)?;
    assign_prev_next(&mut pages, &section_meta);
    let link_map = build_link_map(&pages);
    for page in &mut pages {
        let (toc, body, broken_links) =
            preprocess_body(&page.body, &link_map, page.meta.toc.unwrap_or(true));
        if !broken_links.is_empty() {
            let msg = format!(
                "broken internal link(s) in {}: {}",
                page.source.display(),
                broken_links.join(", ")
            );
            if cfg.fail_on_broken_links {
                bail!(msg);
            } else {
                eprintln!("warning: {msg}");
            }
        }
        page.toc = toc;
        page.processed_body = body;
    }
    let summaries = pages.iter().map(Page::summary).collect::<Vec<_>>();
    let generated = make_generated_pages(&cfg, &out_root, &summaries, &section_meta);
    validate_routes(&pages, &generated)?;
    if skipped.total() > 0 {
        eprintln!(
            "warning: skipped {} page(s): drafts={} future={} expired={}",
            skipped.total(),
            skipped.drafts,
            skipped.future,
            skipped.expired
        );
    }
    let data_path = cache_root.join("site.typ");
    let site_data = site_data_typ(&cfg, &summaries, &generated, &section_meta);
    write_if_changed(&data_path, &site_data)?;
    write_typage_package_data(&root, &cfg, &site_data)?;
    write_theme_package_data(&root, &cfg)?;

    let bundle_path = cache_root.join("bundle.typ");
    write_if_changed(
        &bundle_path,
        &bundle_source(&root, &cfg, &data_path, &pages, &generated, &collections)?,
    )?;
    let bundle_current = pages
        .first()
        .map(page_current_json)
        .transpose()?
        .unwrap_or_else(|| "{}".to_string());
    let bundle_inputs = [("typage_current", bundle_current.as_str())];
    compile_typst(
        &cfg,
        &root,
        &bundle_path,
        &out_root,
        "bundle",
        &cfg.bundle_features,
        "bundle".to_string(),
        &bundle_inputs,
    )?;
    println!("bundle output: {}", out_root.display());
    Ok(())
}

fn discover_sections(content_root: &Path) -> Result<BTreeMap<String, FrontMatter>> {
    let mut sections = BTreeMap::new();
    if !content_root.exists() {
        return Ok(sections);
    }
    for entry in walkdir::WalkDir::new(content_root)
        .into_iter()
        .filter_entry(|e| !is_hidden(e.path()))
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type().is_file() || path.file_name() != Some(OsStr::new("_index.typ")) {
            continue;
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let (meta, _) = split_metadata(path, &raw)
            .with_context(|| format!("failed to parse metadata in {}", path.display()))?;
        let parent = path.parent().unwrap_or(content_root);
        let rel = parent.strip_prefix(content_root).unwrap_or(parent);
        let section = if rel.as_os_str().is_empty() {
            "pages".to_string()
        } else {
            to_posix_path(rel)
        };
        sections.insert(section, meta);
    }
    Ok(sections)
}

fn assign_prev_next(pages: &mut [Page], section_meta: &BTreeMap<String, FrontMatter>) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, page) in pages.iter().enumerate() {
        groups.entry(page.section.clone()).or_default().push(idx);
    }
    for (section, mut order) in groups {
        let sort_by = section_meta
            .get(&section)
            .and_then(|m| m.sort_by.as_deref())
            .unwrap_or("date_desc");
        order.sort_by(|&a, &b| compare_pages_for_sort(&pages[a], &pages[b], sort_by));
        for pos in 0..order.len() {
            let idx = order[pos];
            if pos > 0 {
                let (prev_url, prev_title) = {
                    let prev = &pages[order[pos - 1]];
                    (prev.url.clone(), prev.title.clone())
                };
                pages[idx].prev_url = Some(prev_url);
                pages[idx].prev_title = Some(prev_title);
            }
            if pos + 1 < order.len() {
                let (next_url, next_title) = {
                    let next = &pages[order[pos + 1]];
                    (next.url.clone(), next.title.clone())
                };
                pages[idx].next_url = Some(next_url);
                pages[idx].next_title = Some(next_title);
            }
        }
    }
}

fn compare_pages_for_sort(a: &Page, b: &Page, sort_by: &str) -> std::cmp::Ordering {
    let aw = a.meta.weight.unwrap_or(0);
    let bw = b.meta.weight.unwrap_or(0);
    match sort_by {
        "weight" | "weight_asc" => aw.cmp(&bw).then(a.url.cmp(&b.url)),
        "weight_desc" => bw.cmp(&aw).then(a.url.cmp(&b.url)),
        "date" | "date_desc" => b
            .meta
            .date
            .cmp(&a.meta.date)
            .then(aw.cmp(&bw))
            .then(a.url.cmp(&b.url)),
        "date_asc" => a
            .meta
            .date
            .cmp(&b.meta.date)
            .then(aw.cmp(&bw))
            .then(a.url.cmp(&b.url)),
        "updated" | "updated_desc" => b
            .meta
            .updated
            .cmp(&a.meta.updated)
            .then(b.meta.date.cmp(&a.meta.date))
            .then(a.url.cmp(&b.url)),
        "updated_asc" => a
            .meta
            .updated
            .cmp(&b.meta.updated)
            .then(a.meta.date.cmp(&b.meta.date))
            .then(a.url.cmp(&b.url)),
        "title" | "title_asc" => a.title.cmp(&b.title).then(a.url.cmp(&b.url)),
        "title_desc" => b.title.cmp(&a.title).then(a.url.cmp(&b.url)),
        "url" | "url_asc" => a.url.cmp(&b.url),
        "url_desc" => b.url.cmp(&a.url),
        _ => b
            .meta
            .date
            .cmp(&a.meta.date)
            .then(aw.cmp(&bw))
            .then(a.url.cmp(&b.url)),
    }
}

fn dependency_hash(source: &Path, body: &str) -> Result<String> {
    let source_dir = source.parent().unwrap_or_else(|| Path::new("."));
    let mut visited = BTreeSet::new();
    let mut parts = Vec::new();
    collect_dependencies(source_dir, body, &mut visited, &mut parts)?;
    let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(hash_strs(&refs))
}

fn dependency_diagnostics(source: &Path, body: &str) -> Vec<String> {
    let source_dir = source.parent().unwrap_or_else(|| Path::new("."));
    let mut warnings = Vec::new();
    for dep in extract_dependency_paths(body) {
        let dep_path = normalize_dependency_path(source_dir, &dep);
        if !dep_path.exists() {
            warnings.push(format!(
                "unresolved dependency in {}: {}",
                source.display(),
                dep
            ));
        }
    }
    warnings
}

fn file_with_deps_hash(path: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let dep_hash = dependency_hash(path, &raw)?;
    Ok(hash_strs(&[&raw, &dep_hash]))
}

fn collect_dependencies(
    base_dir: &Path,
    body: &str,
    visited: &mut BTreeSet<PathBuf>,
    parts: &mut Vec<String>,
) -> Result<()> {
    for dep in extract_dependency_paths(body) {
        let dep_path = normalize_dependency_path(base_dir, &dep);
        if !dep_path.exists() || !dep_path.is_file() {
            continue;
        }
        let canonical = fs::canonicalize(&dep_path)
            .with_context(|| format!("failed to canonicalize dependency {}", dep_path.display()))?;
        if !visited.insert(canonical.clone()) {
            continue;
        }
        let bytes = fs::read(&canonical)
            .with_context(|| format!("failed to read dependency {}", canonical.display()))?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        parts.push(format!("{}:{}", canonical.display(), text));
        if canonical.extension() == Some(OsStr::new("typ")) {
            let next_base = canonical.parent().unwrap_or(base_dir);
            collect_dependencies(next_base, &text, visited, parts)?;
        }
    }
    Ok(())
}

fn normalize_dependency_path(base_dir: &Path, dep: &str) -> PathBuf {
    let dep = dep.strip_prefix("./").unwrap_or(dep);
    base_dir.join(dep)
}

fn extract_dependency_paths(body: &str) -> BTreeSet<String> {
    let mut deps = BTreeSet::new();
    let mut raw_fence_len = None;
    for line in body.lines() {
        if let Some(fence_len) = line_raw_fence_len(line) {
            match raw_fence_len {
                Some(open_len) if fence_len >= open_len => raw_fence_len = None,
                None => raw_fence_len = Some(fence_len),
                _ => {}
            }
            continue;
        }
        if raw_fence_len.is_some() {
            continue;
        }
        let line = strip_inline_code_spans(line);
        for marker in ["#import", "#include", "read(", "image("] {
            if let Some(path) = quoted_after(&line, marker) {
                if is_local_typst_path(&path) {
                    deps.insert(path);
                }
            }
        }
    }
    deps
}

fn quoted_after(line: &str, marker: &str) -> Option<String> {
    let start = line.find(marker)? + marker.len();
    let rest = line[start..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let second = rest.find('"')?;
    Some(rest[..second].to_string())
}

fn is_local_typst_path(path: &str) -> bool {
    !(path.starts_with('@')
        || path.starts_with('/')
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("data:"))
}

fn line_raw_fence_len(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let len = trimmed.chars().take_while(|ch| *ch == '`').count();
    (len >= 3).then_some(len)
}

fn strip_inline_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_code = false;
    while let Some(ch) = chars.next() {
        if ch == '`' {
            in_code = !in_code;
            while chars.peek() == Some(&'`') {
                let _ = chars.next();
            }
            continue;
        }
        if !in_code {
            out.push(ch);
        }
    }
    out
}

fn validate_routes(pages: &[Page], generated: &[GeneratedPage]) -> Result<()> {
    let mut seen = BTreeMap::<String, String>::new();
    for page in pages {
        validate_url_path(&page.url)
            .with_context(|| format!("invalid URL for {}", page.source.display()))?;
        insert_route(
            &mut seen,
            &page.url,
            format!("page {}", page.source.display()),
        )?;
        for alias in &page.meta.aliases {
            let alias_url = alias_to_url(alias).with_context(|| {
                format!("invalid alias {:?} in {}", alias, page.source.display())
            })?;
            insert_route(
                &mut seen,
                &alias_url,
                format!("alias {:?} for {}", alias, page.source.display()),
            )?;
        }
    }
    for gen in generated {
        validate_url_path(&gen.url)
            .with_context(|| format!("invalid generated URL {}", gen.url))?;
        insert_route(&mut seen, &gen.url, format!("generated {}", gen.title))?;
    }
    Ok(())
}

fn insert_route(seen: &mut BTreeMap<String, String>, url: &str, label: String) -> Result<()> {
    if let Some(prev) = seen.insert(url.to_string(), label.clone()) {
        bail!("route collision for {url}: {prev} conflicts with {label}");
    }
    Ok(())
}

fn validate_url_path(url: &str) -> Result<()> {
    if !url.starts_with('/') {
        bail!("URL must start with '/': {url}");
    }
    if url.contains('\\') || url.contains('?') || url.contains('#') {
        bail!("URL contains invalid character: {url}");
    }
    for part in url.trim_matches('/').split('/') {
        if part.is_empty() {
            continue;
        }
        if part == "." || part == ".." {
            bail!("URL must not contain path traversal: {url}");
        }
    }
    Ok(())
}

fn alias_to_url(alias: &str) -> Result<String> {
    normalize_directory_url(alias).with_context(|| format!("invalid alias path: {alias}"))
}

fn output_path_for_url(out_root: &Path, url: &str) -> Result<PathBuf> {
    validate_url_path(url)?;
    let clean = url.trim_matches('/');
    let path = if clean.is_empty() {
        out_root.join("index.html")
    } else {
        out_root.join(clean).join("index.html")
    };
    Ok(path)
}

fn public_file_path(out_root: &Path, path: &str) -> Result<PathBuf> {
    let clean = public_file_url(path)?.trim_start_matches('/').to_string();
    Ok(out_root.join(clean))
}

fn public_file_url(path: &str) -> Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.contains('\\')
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.ends_with('/')
    {
        bail!("unsafe public file path: {trimmed}");
    }
    let clean = trimmed.trim_start_matches('/');
    if clean.is_empty() {
        bail!("public file path must not be empty");
    }
    for seg in clean.split('/') {
        validate_url_segment(seg)?;
    }
    Ok(format!("/{clean}"))
}

fn write_alias_pages(out_root: &Path, pages: &[Page]) -> Result<()> {
    for page in pages {
        for alias in &page.meta.aliases {
            let alias_url = alias_to_url(alias)?;
            let path = output_path_for_url(out_root, &alias_url)?;
            let html = format!(
                "<!doctype html><meta charset=\"utf-8\"><title>Redirect</title><link rel=\"canonical\" href=\"{}\"><meta http-equiv=\"refresh\" content=\"0; url={}\"><p><a href=\"{}\">Moved</a></p>\n",
                escape_html(&page.canonical_url), escape_html(&page.canonical_url), escape_html(&page.canonical_url)
            );
            write_if_changed(&path, &html)?;
        }
    }
    Ok(())
}

fn desired_public_outputs(
    cfg: &Config,
    static_roots: &[PathBuf],
    out_root: &Path,
    pages: &[Page],
    generated: &[GeneratedPage],
    build_pdf_flag: bool,
) -> Result<BTreeSet<PathBuf>> {
    let mut desired = BTreeSet::new();
    for page in pages {
        desired.insert(page.output_html.clone());
        if build_pdf_flag || cfg.build_pdf || page.meta.build_pdf.unwrap_or(false) {
            desired.insert(page.output_pdf.clone());
        }
        for alias in &page.meta.aliases {
            let alias_url = alias_to_url(alias)?;
            desired.insert(output_path_for_url(out_root, &alias_url)?);
        }
    }
    for gen in generated {
        desired.insert(gen.output_html.clone());
    }
    for static_root in static_roots {
        if static_root.exists() {
            for entry in walkdir::WalkDir::new(static_root) {
                let entry = entry?;
                if entry.file_type().is_file() {
                    let rel = entry.path().strip_prefix(static_root)?;
                    desired.insert(out_root.join(rel));
                }
            }
        }
    }
    if cfg.sitemap && !cfg.base_url.trim().is_empty() {
        desired.insert(out_root.join("sitemap.xml"));
    }
    if cfg.feed {
        desired.insert(public_file_path(out_root, &cfg.feed_path)?);
        if cfg.atom_feed {
            desired.insert(public_file_path(out_root, &cfg.atom_path)?);
        }
        for feed in &cfg.feeds {
            desired.insert(public_file_path(out_root, &feed.path)?);
        }
    }
    if cfg.robots {
        desired.insert(out_root.join("robots.txt"));
    }
    if cfg.search.enabled {
        desired.insert(out_root.join("search_index.json"));
    }
    Ok(desired)
}

fn cleanup_public_outputs(out_root: &Path, desired: &BTreeSet<PathBuf>) -> Result<()> {
    if !out_root.exists() {
        return Ok(());
    }
    let mut dirs = Vec::new();
    for entry in walkdir::WalkDir::new(out_root).contents_first(true) {
        let entry = entry?;
        let path = entry.path().to_path_buf();
        if entry.file_type().is_file() || entry.file_type().is_symlink() {
            if !desired.contains(&path) {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove stale output {}", path.display()))?;
            }
        } else if entry.file_type().is_dir() && path != out_root {
            dirs.push(path);
        }
    }
    for dir in dirs {
        if fs::read_dir(&dir)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(&dir);
        }
    }
    Ok(())
}

fn write_sitemap(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
    generated: &[GeneratedPage],
) -> Result<()> {
    if cfg.base_url.trim().is_empty() {
        eprintln!("warning: sitemap.xml skipped because base_url is empty");
        return Ok(());
    }
    let mut urls: BTreeMap<String, Option<String>> = BTreeMap::new();
    for page in pages {
        let entry = urls.entry(page.url.clone()).or_insert(None);
        *entry = newest_date(
            entry.clone(),
            page.updated.clone().or_else(|| page.date.clone()),
        );
    }
    for gen in generated {
        let latest = latest_page_date(&gen.items);
        let entry = urls.entry(gen.url.clone()).or_insert(None);
        *entry = newest_date(entry.clone(), latest);
    }
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for (url, lastmod) in urls {
        xml.push_str("  <url>\n    <loc>");
        xml.push_str(&escape_xml(&absolute_url(cfg, &url)));
        xml.push_str("</loc>\n");
        if let Some(lastmod) = lastmod {
            xml.push_str("    <lastmod>");
            xml.push_str(&escape_xml(&lastmod));
            xml.push_str("</lastmod>\n");
        }
        xml.push_str("  </url>\n");
    }
    xml.push_str("</urlset>\n");
    write_if_changed(&out_root.join("sitemap.xml"), &xml)
}

fn write_configured_feeds(cfg: &Config, out_root: &Path, pages: &[PageSummary]) -> Result<()> {
    let primary = FeedConfig {
        path: cfg.feed_path.clone(),
        title: cfg.feed_title.clone(),
        description: cfg.feed_description.clone(),
        link: cfg.feed_link.clone(),
        section: None,
        sections: cfg.feed_sections.clone(),
        limit: cfg.feed_limit,
    };
    write_rss_feed(cfg, out_root, pages, &primary)?;
    for feed in &cfg.feeds {
        write_rss_feed(cfg, out_root, pages, feed)?;
    }
    Ok(())
}

fn feed_items(cfg: &Config, pages: &[PageSummary], spec: &FeedConfig) -> Vec<PageSummary> {
    let mut sections = BTreeSet::new();
    if let Some(section) = &spec.section {
        if !section.trim().is_empty() {
            sections.insert(section.clone());
        }
    }
    for section in &spec.sections {
        if !section.trim().is_empty() {
            sections.insert(section.clone());
        }
    }
    if sections.is_empty() {
        for section in &cfg.feed_sections {
            if !section.trim().is_empty() {
                sections.insert(section.clone());
            }
        }
    }

    let mut items = pages
        .iter()
        .filter(|page| sections.is_empty() || sections.contains(&page.section))
        .cloned()
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.date.cmp(&a.date).then(a.url.cmp(&b.url)));
    if spec.limit > 0 {
        items.truncate(spec.limit);
    }
    items
}

fn feed_description<'a>(cfg: &'a Config, spec: &'a FeedConfig) -> &'a str {
    spec.description
        .as_deref()
        .or_else(|| cfg.feed_description.as_deref())
        .or_else(|| cfg.extra.get("description").and_then(|v| v.as_str()))
        .unwrap_or(&cfg.title)
}

fn feed_title<'a>(cfg: &'a Config, spec: &'a FeedConfig) -> &'a str {
    spec.title
        .as_deref()
        .or_else(|| cfg.feed_title.as_deref())
        .unwrap_or(&cfg.title)
}

fn write_rss_feed(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
    spec: &FeedConfig,
) -> Result<()> {
    let items = feed_items(cfg, pages, spec);
    let link = if spec.link.trim().is_empty() {
        "/"
    } else {
        spec.link.trim()
    };
    let self_url = public_file_url(&spec.path)?;
    let latest = latest_page_date(&items).map(|d| rss_date(&d));
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rss version=\"2.0\"><channel><title>{}</title><link>{}</link><description>{}</description>\n",
        escape_xml(feed_title(cfg, spec)),
        escape_xml(&absolute_url(cfg, link)),
        escape_xml(feed_description(cfg, spec))
    );
    xml.push_str(&format!("<atom:link xmlns:atom=\"http://www.w3.org/2005/Atom\" href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>\n", escape_xml(&absolute_url(cfg, &self_url))));
    if let Some(date) = latest {
        xml.push_str(&format!(
            "<lastBuildDate>{}</lastBuildDate>\n",
            escape_xml(&date)
        ));
    }
    for page in items {
        xml.push_str("<item>");
        xml.push_str(&format!("<title>{}</title>", escape_xml(&page.title)));
        xml.push_str(&format!(
            "<link>{}</link>",
            escape_xml(&absolute_url(cfg, &page.url))
        ));
        xml.push_str(&format!(
            "<guid isPermaLink=\"true\">{}</guid>",
            escape_xml(&absolute_url(cfg, &page.url))
        ));
        if let Some(date) = &page.date {
            xml.push_str(&format!(
                "<pubDate>{}</pubDate>",
                escape_xml(&rss_date(date))
            ));
        }
        let desc = page.excerpt.as_ref().or(page.description.as_ref());
        if let Some(desc) = desc {
            xml.push_str(&format!("<description>{}</description>", escape_xml(desc)));
        }
        xml.push_str("</item>\n");
    }
    xml.push_str("</channel></rss>\n");
    write_if_changed(&public_file_path(out_root, &spec.path)?, &xml)
}

fn write_atom_feed(cfg: &Config, out_root: &Path, pages: &[PageSummary]) -> Result<()> {
    let spec = FeedConfig {
        path: cfg.atom_path.clone(),
        title: cfg.feed_title.clone(),
        description: cfg.feed_description.clone(),
        link: cfg.feed_link.clone(),
        section: None,
        sections: cfg.feed_sections.clone(),
        limit: cfg.feed_limit,
    };
    let items = feed_items(cfg, pages, &spec);
    let self_url = public_file_url(&cfg.atom_path)?;
    let updated = latest_page_date(&items)
        .map(|d| atom_date(&d))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n  <title>{}</title>\n  <id>{}</id>\n  <link href=\"{}\"/>\n  <link rel=\"self\" href=\"{}\"/>\n  <updated>{}</updated>\n",
        escape_xml(feed_title(cfg, &spec)),
        escape_xml(&absolute_url(cfg, "/")),
        escape_xml(&absolute_url(cfg, "/")),
        escape_xml(&absolute_url(cfg, &self_url)),
        escape_xml(&updated),
    );
    for page in items {
        let url = absolute_url(cfg, &page.url);
        let updated = page
            .updated
            .as_ref()
            .or(page.date.as_ref())
            .map(|d| atom_date(d))
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
        xml.push_str("  <entry>\n");
        xml.push_str(&format!("    <title>{}</title>\n", escape_xml(&page.title)));
        xml.push_str(&format!("    <id>{}</id>\n", escape_xml(&url)));
        xml.push_str(&format!("    <link href=\"{}\"/>\n", escape_xml(&url)));
        xml.push_str(&format!(
            "    <updated>{}</updated>\n",
            escape_xml(&updated)
        ));
        let summary = page.excerpt.as_ref().or(page.description.as_ref());
        if let Some(summary) = summary {
            xml.push_str(&format!("    <summary>{}</summary>\n", escape_xml(summary)));
        }
        xml.push_str("  </entry>\n");
    }
    xml.push_str("</feed>\n");
    write_if_changed(&public_file_path(out_root, &cfg.atom_path)?, &xml)
}

fn write_robots(cfg: &Config, out_root: &Path) -> Result<()> {
    let mut text = String::from("User-agent: *\nAllow: /\n");
    if cfg.sitemap && !cfg.base_url.trim().is_empty() {
        text.push('\n');
        text.push_str("Sitemap: ");
        text.push_str(&absolute_url(cfg, "/sitemap.xml"));
        text.push('\n');
    }
    write_if_changed(&out_root.join("robots.txt"), &text)
}

fn latest_page_date(items: &[PageSummary]) -> Option<String> {
    items
        .iter()
        .filter_map(|page| page.updated.clone().or_else(|| page.date.clone()))
        .max()
}

fn newest_date(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[derive(Serialize)]
struct SearchIndex {
    version: u32,
    config: SearchIndexConfig,
    pages: Vec<SearchEntry>,
}

#[derive(Serialize)]
struct SearchIndexConfig {
    mode: String,
    ngram: usize,
    compact: bool,
}

#[derive(Serialize)]
struct SearchEntry {
    title: String,
    url: String,
    description: Option<String>,
    date: Option<String>,
    updated: Option<String>,
    weight: Option<i64>,
    section: String,
    tags: Vec<String>,
    categories: Vec<String>,
    excerpt: String,
    /// Full body text. Empty when `search.compact = true`.
    text: String,
    tokens: Vec<String>,
    headings: Vec<SearchHeading>,
    fields: SearchFields,
}

#[derive(Serialize)]
struct SearchHeading {
    level: usize,
    id: String,
    text: String,
    tokens: Vec<String>,
}

#[derive(Serialize)]
struct SearchFields {
    title: Vec<String>,
    headings: Vec<String>,
    body: Vec<String>,
    taxonomies: Vec<String>,
}

fn search_diagnostics(cfg: &SearchConfig, pages: &[Page]) -> Vec<String> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let mode = cfg.mode.as_str();
    if !matches!(mode, "auto" | "latin" | "cjk" | "ngram" | "ngram-only") {
        warnings.push(format!(
            "unknown search.mode `{}`; expected auto, latin, cjk, ngram, or ngram-only",
            cfg.mode
        ));
    }
    if cfg.ngram == 0 || cfg.ngram > 8 {
        warnings.push(format!(
            "search.ngram={} will be clamped to {}",
            cfg.ngram,
            normalized_ngram(cfg)
        ));
    }
    let mut indexed_chars = 0usize;
    let mut cjk_pages = 0usize;
    for page in pages {
        let text = plain_text_for_search(&page.processed_body);
        indexed_chars += page.title.chars().count();
        indexed_chars += page
            .meta
            .description
            .as_deref()
            .unwrap_or("")
            .chars()
            .count();
        indexed_chars += text.chars().count();
        if text.chars().any(is_cjk) || page.title.chars().any(is_cjk) {
            cjk_pages += 1;
        }
    }
    println!(
        "search: {} mode={} ngram={} compact={} pages={} cjk_pages={} indexed_chars≈{}",
        if cfg.enabled { "enabled" } else { "disabled" },
        cfg.mode,
        normalized_ngram(cfg),
        cfg.compact,
        pages.len(),
        cjk_pages,
        indexed_chars
    );
    if cjk_pages > 0 && matches!(mode, "latin") {
        warnings.push(
            "CJK text detected but search.mode = \"latin\"; consider mode = \"cjk\" or \"auto\""
                .to_string(),
        );
    }
    if !cfg.compact && indexed_chars > 500_000 {
        warnings.push(format!(
            "search index may be large ({} indexed chars); consider [search] compact = true",
            indexed_chars
        ));
    }
    warnings
}

fn write_search_index(out_root: &Path, pages: &[Page], cfg: &SearchConfig) -> Result<()> {
    let entries = pages
        .iter()
        .map(|page| {
            let text = plain_text_for_search(&page.processed_body);
            let excerpt = page
                .meta
                .excerpt
                .clone()
                .or_else(|| page.meta.description.clone())
                .unwrap_or_else(|| make_excerpt(&text, 180));
            let title_tokens = tokenize_search_text(&page.title, cfg);
            let heading_entries = if cfg.include_headings {
                page.toc
                    .iter()
                    .map(|item| SearchHeading {
                        level: item.level,
                        id: item.id.clone(),
                        text: truncate_chars(&item.text, cfg.max_heading_chars),
                        tokens: tokenize_search_text(&item.text, cfg),
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let heading_tokens = dedupe_tokens(
                heading_entries
                    .iter()
                    .flat_map(|h| h.tokens.iter().cloned())
                    .collect::<Vec<_>>(),
                cfg.max_tokens,
            );
            let body_tokens = if cfg.include_body {
                tokenize_search_text(&text, cfg)
            } else {
                Vec::new()
            };
            let taxonomy_text = taxonomy_search_text(page, cfg);
            let taxonomy_tokens = tokenize_search_text(&taxonomy_text, cfg);
            let mut all_tokens = Vec::new();
            all_tokens.extend(title_tokens.clone());
            all_tokens.extend(tokenize_search_text(
                page.meta.description.as_deref().unwrap_or(""),
                cfg,
            ));
            all_tokens.extend(tokenize_search_text(&excerpt, cfg));
            all_tokens.extend(heading_tokens.clone());
            all_tokens.extend(body_tokens.clone());
            all_tokens.extend(taxonomy_tokens.clone());
            let tokens = dedupe_tokens(all_tokens, cfg.max_tokens);
            SearchEntry {
                title: page.title.clone(),
                url: page.url.clone(),
                description: page.meta.description.clone(),
                date: page.meta.date.clone(),
                updated: page.meta.updated.clone(),
                weight: page.meta.weight,
                section: page.section.clone(),
                tags: page.meta.tags.clone(),
                categories: page.meta.categories.clone(),
                excerpt,
                text: if cfg.compact {
                    String::new()
                } else {
                    truncate_chars(&text, cfg.max_body_chars)
                },
                tokens,
                headings: heading_entries,
                fields: SearchFields {
                    title: title_tokens,
                    headings: heading_tokens,
                    body: body_tokens,
                    taxonomies: taxonomy_tokens,
                },
            }
        })
        .collect::<Vec<_>>();
    let index = SearchIndex {
        version: 2,
        config: SearchIndexConfig {
            mode: cfg.mode.clone(),
            ngram: normalized_ngram(cfg),
            compact: cfg.compact,
        },
        pages: entries,
    };
    let json = serde_json::to_string_pretty(&index)?;
    write_if_changed(&out_root.join("search_index.json"), &json)
}

fn taxonomy_search_text(page: &Page, cfg: &SearchConfig) -> String {
    if !cfg.include_tags && !cfg.include_taxonomies {
        return String::new();
    }
    let mut parts = Vec::new();
    if cfg.include_tags {
        parts.extend(page.meta.tags.iter().cloned());
        parts.extend(page.meta.categories.iter().cloned());
    }
    if cfg.include_taxonomies {
        for (key, field) in &page.meta.fields {
            if matches!(key.as_str(), "tags" | "categories") {
                continue;
            }
            let Some(value) = field.value.clone() else {
                continue;
            };
            match value {
                toml::Value::String(s) => parts.push(s),
                toml::Value::Array(xs) => {
                    for x in xs {
                        if let Some(s) = x.as_str() {
                            parts.push(s.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    parts.join(" ")
}

#[cfg(test)]
fn search_tokens(page: &Page, text: &str, excerpt: &str) -> Vec<String> {
    let cfg = SearchConfig::default();
    let mut all = Vec::new();
    all.extend(tokenize_search_text(&page.title, &cfg));
    all.extend(tokenize_search_text(
        page.meta.description.as_deref().unwrap_or(""),
        &cfg,
    ));
    all.extend(tokenize_search_text(excerpt, &cfg));
    all.extend(tokenize_search_text(text, &cfg));
    all.extend(tokenize_search_text(
        &taxonomy_search_text(page, &cfg),
        &cfg,
    ));
    dedupe_tokens(all, cfg.max_tokens)
}

fn tokenize_search_text(text: &str, cfg: &SearchConfig) -> Vec<String> {
    let mode = cfg.mode.as_str();
    let include_latin = mode != "ngram-only";
    let include_cjk = mode == "auto" || mode == "cjk" || mode == "ngram" || mode == "ngram-only";
    let mut tokens = Vec::<String>::new();
    let mut latin = String::new();
    let mut cjk = String::new();

    for ch in text.chars() {
        if is_cjk(ch) {
            push_latin_token_vec(&mut tokens, &mut latin);
            if include_cjk {
                cjk.push(ch);
            }
        } else if ch.is_alphanumeric() || ch == '_' {
            push_cjk_ngrams_vec(&mut tokens, &mut cjk, cfg);
            if include_latin {
                for lower in ch.to_lowercase() {
                    latin.push(lower);
                }
            }
        } else {
            push_latin_token_vec(&mut tokens, &mut latin);
            push_cjk_ngrams_vec(&mut tokens, &mut cjk, cfg);
        }
    }
    push_latin_token_vec(&mut tokens, &mut latin);
    push_cjk_ngrams_vec(&mut tokens, &mut cjk, cfg);
    dedupe_tokens(tokens, cfg.max_tokens)
}

fn dedupe_tokens(tokens: Vec<String>, max_tokens: usize) -> Vec<String> {
    let mut set = BTreeSet::new();
    let mut out = Vec::new();
    let limit = max_tokens.max(1);
    for token in tokens {
        if token.is_empty() {
            continue;
        }
        if set.insert(token.clone()) {
            out.push(token);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn push_latin_token_vec(out: &mut Vec<String>, current: &mut String) {
    let token = current.trim_matches('_').to_string();
    current.clear();
    if token.chars().count() >= 2 {
        out.push(token);
    }
}

fn push_cjk_ngrams_vec(out: &mut Vec<String>, current: &mut String, cfg: &SearchConfig) {
    if current.is_empty() {
        return;
    }
    let chars = current.chars().collect::<Vec<_>>();
    current.clear();
    let n = normalized_ngram(cfg);
    if chars.len() < n {
        out.push(chars.into_iter().collect::<String>());
        return;
    }
    for window in chars.windows(n) {
        out.push(window.iter().collect::<String>());
    }
}

fn normalized_ngram(cfg: &SearchConfig) -> usize {
    cfg.ngram.clamp(1, 8)
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x309f // Hiragana
        | 0x30a0..=0x30ff // Katakana
        | 0x3400..=0x4dbf // CJK Extension A
        | 0x4e00..=0x9fff // CJK Unified Ideographs
        | 0xf900..=0xfaff // CJK Compatibility Ideographs
        | 0xff66..=0xff9f // Halfwidth Katakana
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

fn make_excerpt(text: &str, max_chars: usize) -> String {
    truncate_chars(text, max_chars).trim().to_string()
}

fn plain_text_for_search(body: &str) -> String {
    let mut out = String::new();
    let mut in_raw_block = false;
    let mut in_code = false;
    let mut skip_command = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_raw_block = !in_raw_block;
            continue;
        }
        if in_raw_block {
            continue;
        }
        for ch in line.chars() {
            if ch == '`' {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            if ch == '#' {
                skip_command = true;
                continue;
            }
            if skip_command {
                if ch.is_whitespace()
                    || ch == '['
                    || ch == ']'
                    || ch == '{'
                    || ch == '}'
                    || ch == ')'
                {
                    skip_command = false;
                }
                continue;
            }
            match ch {
                '=' | '*' | '_' | '[' | ']' | '{' | '}' | '(' | ')' | ',' => out.push(' '),
                _ => out.push(ch),
            }
        }
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn atom_date(date: &str) -> String {
    if date.split('-').count() == 3 {
        format!("{date}T00:00:00Z")
    } else {
        date.to_string()
    }
}

fn rss_date(date: &str) -> String {
    let parts = date.split('-').collect::<Vec<_>>();
    if parts.len() == 3 {
        if let Ok(month) = parts[1].parse::<usize>() {
            let month_name = [
                "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
                "Dec",
            ]
            .get(month)
            .copied()
            .unwrap_or("");
            if !month_name.is_empty() {
                return format!("{} {} {} 00:00:00 +0000", parts[2], month_name, parts[0]);
            }
        }
    }
    date.to_string()
}

fn absolute_url(cfg: &Config, path: &str) -> String {
    if cfg.base_url.is_empty() {
        path.to_string()
    } else {
        format!("{}{}", cfg.base_url.trim_end_matches('/'), path)
    }
}

fn escape_html(s: &str) -> String {
    escape_xml(s)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Debug, Clone)]
struct PageAddress {
    url: String,
    canonical_url: String,
    slug: String,
    path: String,
    file_path: String,
}

fn discover_pages(
    root: &Path,
    cfg: &Config,
    content_root: &Path,
    out_root: &Path,
    drafts: bool,
    collections: &ContentCollections,
) -> Result<(Vec<Page>, SkipStats)> {
    let mut pages = Vec::new();
    let mut skipped = SkipStats::default();
    if !content_root.exists() {
        return Ok((pages, skipped));
    }
    for entry in walkdir::WalkDir::new(content_root)
        .into_iter()
        .filter_entry(|e| !is_hidden(e.path()))
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type().is_file() || !is_typst_file(path) {
            continue;
        }
        if path == content_root.join("config.typ") {
            continue;
        }
        if path.file_name() == Some(OsStr::new("_index.typ")) {
            continue;
        }
        if is_content_partial(path) {
            continue;
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let (mut meta, body) = split_metadata(path, &raw)
            .with_context(|| format!("failed to parse metadata in {}", path.display()))?;
        if meta.lang.as_deref().unwrap_or("").trim().is_empty() {
            meta.lang = Some(cfg.lang.clone());
        }
        if meta.draft && !drafts {
            skipped.drafts += 1;
            continue;
        }
        if !cfg.build_future && is_future_date(meta.date.as_deref()) {
            skipped.future += 1;
            continue;
        }
        if !cfg.build_expired && is_expired_date(meta.expires.as_deref()) {
            skipped.expired += 1;
            continue;
        }
        let rel = path.strip_prefix(content_root)?.to_path_buf();
        let title = meta.title.clone().unwrap_or_else(|| default_title(path));
        let section = meta.section.clone().unwrap_or_else(|| infer_section(&rel));
        validate_page_metadata_schema(path, &section, &meta, collections)?;
        let address = page_address(cfg, &rel, &section, &meta)?;
        let output_html = out_root
            .join(address.url.trim_start_matches('/'))
            .join("index.html");
        let output_pdf = out_root
            .join(address.url.trim_start_matches('/'))
            .join("index.pdf");
        let template = meta
            .template
            .clone()
            .unwrap_or_else(|| cfg.default_template.clone());
        let parent_section = parent_section_name(&section);
        let ancestors = section_ancestors(&section);
        pages.push(Page {
            source: path.to_path_buf(),
            rel,
            body,
            processed_body: String::new(),
            meta,
            title,
            url: address.url,
            canonical_url: address.canonical_url,
            slug: address.slug,
            path: address.path,
            file_path: address.file_path,
            output_html,
            output_pdf,
            template,
            section,
            parent_section,
            ancestors,
            toc: Vec::new(),
            hash: String::new(),
            prev_title: None,
            prev_url: None,
            next_title: None,
            next_url: None,
        });
    }
    pages.sort_by(|a, b| a.url.cmp(&b.url));
    let _ = root;
    Ok((pages, skipped))
}

fn validate_page_metadata_schema(
    path: &Path,
    section: &str,
    meta: &FrontMatter,
    collections: &ContentCollections,
) -> Result<()> {
    let Some(schema) = collections.schema_for(section) else {
        return Ok(());
    };
    for key in meta.fields.keys() {
        if !schema.fields.contains_key(key) {
            bail!(
                "{}: metadata field `{key}` is not declared in collection schema `{}`",
                path.display(),
                section.split('/').next().unwrap_or(section)
            );
        }
    }
    for (name, field_schema) in &schema.fields {
        let value = page_metadata_owned_value(meta, name);
        if value.is_none() && !field_schema.optional {
            bail!(
                "{}: missing required metadata field `{name}` for collection `{}`",
                path.display(),
                section.split('/').next().unwrap_or(section)
            );
        }
        if let Some(value) = value {
            validate_toml_schema_value(path, name, &value, field_schema)?;
        }
    }
    Ok(())
}

fn page_metadata_owned_value(meta: &FrontMatter, name: &str) -> Option<toml::Value> {
    meta.fields
        .get(name)
        .and_then(|field| field.value.clone())
        .or_else(|| core_metadata_owned_value(meta, name))
}

fn core_metadata_owned_value(meta: &FrontMatter, name: &str) -> Option<toml::Value> {
    match name {
        "title" => meta.title.clone().map(toml::Value::String),
        "description" => meta.description.clone().map(toml::Value::String),
        "date" => meta.date.clone().map(toml::Value::String),
        "updated" => meta.updated.clone().map(toml::Value::String),
        "expires" => meta.expires.clone().map(toml::Value::String),
        "weight" => meta.weight.map(toml::Value::Integer),
        "lang" => meta.lang.clone().map(toml::Value::String),
        "draft" => Some(toml::Value::Boolean(meta.draft)),
        "slug" => meta.slug.clone().map(toml::Value::String),
        "path" | "permalink" => meta.permalink.clone().map(toml::Value::String),
        "template" => meta.template.clone().map(toml::Value::String),
        "section" => meta.section.clone().map(toml::Value::String),
        "tags" => Some(toml::Value::Array(
            meta.tags.iter().cloned().map(toml::Value::String).collect(),
        )),
        "categories" => Some(toml::Value::Array(
            meta.categories
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        )),
        "aliases" => Some(toml::Value::Array(
            meta.aliases
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        )),
        "build_pdf" => meta.build_pdf.map(toml::Value::Boolean),
        "excerpt" => meta.excerpt.clone().map(toml::Value::String),
        "toc" => meta.toc.map(toml::Value::Boolean),
        "sort_by" => meta.sort_by.clone().map(toml::Value::String),
        "paginate_by" => meta
            .paginate_by
            .map(|value| toml::Value::Integer(value as i64)),
        _ => None,
    }
}

fn validate_toml_schema_value(
    path: &Path,
    name: &str,
    value: &toml::Value,
    schema: &MetadataFieldSchema,
) -> Result<()> {
    if !toml_value_matches_schema(value, schema) {
        bail!(
            "{}: metadata field `{name}` does not match collection schema",
            path.display()
        );
    }
    Ok(())
}

fn toml_value_matches_schema(value: &toml::Value, schema: &MetadataFieldSchema) -> bool {
    match &schema.kind {
        MetadataFieldKind::Any => true,
        MetadataFieldKind::Builtin(name) => toml_value_matches_builtin(value, name),
        MetadataFieldKind::Array(inner) => match value {
            toml::Value::Array(values) => values
                .iter()
                .all(|value| toml_value_matches_schema(value, inner)),
            _ => false,
        },
        MetadataFieldKind::Object(fields) if fields.contains_key("*") => {
            matches!(value, toml::Value::Table(_))
        }
        MetadataFieldKind::Object(fields) => match value {
            toml::Value::Table(table) => fields.iter().all(|(key, field_schema)| {
                table
                    .get(key)
                    .map(|value| toml_value_matches_schema(value, field_schema))
                    .unwrap_or(field_schema.optional)
            }),
            _ => false,
        },
        MetadataFieldKind::Union(options) => options
            .iter()
            .any(|option| toml_value_matches_schema(value, option)),
    }
}

fn toml_value_matches_builtin(value: &toml::Value, name: &str) -> bool {
    match name {
        "str" | "url" | "datetime" | "date" | "path" | "label" | "regex" | "symbol" | "version" => {
            matches!(value, toml::Value::String(_) | toml::Value::Datetime(_))
        }
        "bool" => matches!(value, toml::Value::Boolean(_)),
        "int" => matches!(value, toml::Value::Integer(_)),
        "float" | "decimal" => matches!(value, toml::Value::Float(_)),
        "number" => matches!(value, toml::Value::Integer(_) | toml::Value::Float(_)),
        "array" => matches!(value, toml::Value::Array(_)),
        "dictionary" => matches!(value, toml::Value::Table(_)),
        _ => true,
    }
}

#[derive(Debug)]
struct TypstMetadataDirective {
    range: std::ops::Range<usize>,
    meta: FrontMatter,
}

fn split_metadata(path: &Path, raw: &str) -> Result<(FrontMatter, String)> {
    let normalized = raw.replace("\r\n", "\n");
    if let Some(rest) = normalized.strip_prefix("---\n") {
        let Some(end) = rest.find("\n---") else {
            bail!(
                "{}:1:1: TOML front matter starts with --- but has no closing ---",
                path.display()
            );
        };
        let fm_raw = &rest[..end];
        let mut body_start = "---\n".len() + end + "\n---".len();
        while normalized[body_start..].starts_with('\n') {
            body_start += 1;
        }
        let body = normalized[body_start..].to_string();
        let directives = find_typst_metadata_directives(path, &normalized, &body, body_start)?;
        if let Some(directive) = directives.first() {
            bail!(
                "{}: cannot combine TOML front matter with Typst metadata directive at {}",
                path.display(),
                metadata_location(path, &normalized, directive.range.start)
            );
        }
        let mut meta = toml::from_str::<FrontMatter>(fm_raw)
            .with_context(|| format!("{}: failed to parse TOML front matter", path.display()))?;
        normalize_frontmatter_fields(path, &mut meta)?;
        apply_metadata_aliases(&mut meta);
        return Ok((meta, body));
    }

    let directives = find_typst_metadata_directives(path, &normalized, &normalized, 0)?;
    match directives.len() {
        0 => Ok((FrontMatter::default(), raw.to_string())),
        1 => {
            let directive = directives.into_iter().next().unwrap();
            let body = remove_metadata_directive(&normalized, directive.range);
            let mut meta = directive.meta;
            apply_metadata_aliases(&mut meta);
            Ok((meta, body))
        }
        _ => {
            let directive = &directives[1];
            bail!(
                "{}: multiple Typst metadata directives found; keep only one `#show: page.with(...)` or `#show: project.with(...)` (second at {})",
                path.display(),
                metadata_location(path, &normalized, directive.range.start)
            );
        }
    }
}

fn normalize_frontmatter_fields(path: &Path, meta: &mut FrontMatter) -> Result<()> {
    let flattened = std::mem::take(&mut meta.flattened_fields);
    for (name, value) in flattened {
        if name == "extra" {
            bail!(
                "{}: TOML `[extra]` is no longer supported; declare collection fields directly",
                path.display()
            );
        }
        validate_metadata_field_name(path, &name)?;
        meta.fields.insert(
            name,
            MetadataField {
                typst: typst_value(&value),
                value: Some(value),
            },
        );
    }
    Ok(())
}

fn validate_metadata_field_name(path: &Path, name: &str) -> Result<()> {
    if is_ident(name) {
        Ok(())
    } else {
        bail!(
            "{}: metadata field `{name}` must be a valid Typst identifier",
            path.display()
        );
    }
}

fn apply_metadata_aliases(meta: &mut FrontMatter) {
    if meta.date.is_none() {
        meta.date = metadata_field_string(meta.fields.get("publishedDate"));
    }
    if meta.updated.is_none() {
        meta.updated = metadata_field_string(meta.fields.get("updatedDate"));
    }
}

fn metadata_field_string(field: Option<&MetadataField>) -> Option<String> {
    match field.and_then(|field| field.value.as_ref()) {
        Some(toml::Value::String(value)) => Some(value.clone()),
        Some(toml::Value::Datetime(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn load_content_collections(content_root: &Path) -> Result<ContentCollections> {
    let path = content_root.join("config.typ");
    if !path.exists() {
        return Ok(ContentCollections::default());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let root = typst_syntax::parse(&raw);
    let linked = LinkedNode::new(&root);
    let mut collections = None;
    for child in linked.children() {
        let Some(binding) = child.get().cast::<ast::LetBinding>() else {
            continue;
        };
        let names = binding
            .kind()
            .bindings()
            .into_iter()
            .map(|ident| ident.as_str().to_string())
            .collect::<Vec<_>>();
        if !names.iter().any(|name| name == "collections") {
            continue;
        }
        if collections.is_some() {
            bail!("{}: duplicate `collections` binding", path.display());
        }
        let Some(init) = binding.init() else {
            bail!("{}: `collections` must be initialized", path.display());
        };
        collections = Some(parse_collections_config(
            &path,
            &raw,
            child.range().start,
            init,
        )?);
    }
    Ok(ContentCollections {
        collections: collections.unwrap_or_default(),
    })
}

fn parse_collections_config(
    path: &Path,
    source: &str,
    offset: usize,
    expr: ast::Expr,
) -> Result<BTreeMap<String, CollectionSchema>> {
    let mut collections = BTreeMap::new();
    for (name, value) in metadata_dict_items(path, source, offset, "collections", expr)? {
        validate_metadata_field_name(path, &name)?;
        collections.insert(name, parse_collection_schema(path, source, offset, &value)?);
    }
    Ok(collections)
}

fn parse_collection_schema<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    expr: &ast::Expr<'a>,
) -> Result<CollectionSchema> {
    if let ast::Expr::FuncCall(call) = expr {
        if is_schema_function_call(*call, "collection") {
            for arg in call.args().items() {
                if let ast::Arg::Named(named) = arg {
                    if named.name().as_str() == "schema" {
                        return parse_schema_object(path, source, offset, named.expr());
                    }
                }
            }
            bail!(
                "{}: collection.with(...) must include `schema: (...)`",
                metadata_location(path, source, offset)
            );
        }
    }

    for (name, value) in metadata_dict_items(path, source, offset, "collection", *expr)? {
        if name == "schema" {
            return parse_schema_object(path, source, offset, value);
        }
    }
    parse_schema_object(path, source, offset, *expr)
}

fn parse_schema_object<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    expr: ast::Expr<'a>,
) -> Result<CollectionSchema> {
    let mut fields = BTreeMap::new();
    for (name, value) in metadata_dict_items(path, source, offset, "schema", expr)? {
        validate_metadata_field_name(path, &name)?;
        fields.insert(name, parse_schema_field(path, source, offset, value)?);
    }
    Ok(CollectionSchema { fields })
}

fn parse_schema_field<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    expr: ast::Expr<'a>,
) -> Result<MetadataFieldSchema> {
    let expr = match expr {
        ast::Expr::Parenthesized(value) => value.expr(),
        other => other,
    };
    match expr {
        ast::Expr::Ident(ident) => Ok(MetadataFieldSchema {
            optional: false,
            kind: schema_builtin_kind(ident.as_str()),
        }),
        ast::Expr::FuncCall(call) => parse_schema_call(path, source, offset, call),
        unsupported => bail!(
            "{}: unsupported schema expression `{}`",
            metadata_location(path, source, offset),
            concise_expr(unsupported)
        ),
    }
}

fn parse_schema_call<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    call: ast::FuncCall<'a>,
) -> Result<MetadataFieldSchema> {
    let name = schema_call_name(call).ok_or_else(|| {
        anyhow::anyhow!(
            "{}: unsupported schema call",
            metadata_location(path, source, offset)
        )
    })?;
    let args = call.args().items().collect::<Vec<_>>();
    match name {
        "optional" => {
            let inner = parse_single_schema_arg(path, source, offset, &args, "optional")?;
            Ok(MetadataFieldSchema {
                optional: true,
                kind: inner.kind,
            })
        }
        "array" => {
            let inner = if args.is_empty() {
                MetadataFieldSchema {
                    optional: false,
                    kind: MetadataFieldKind::Any,
                }
            } else {
                parse_single_schema_arg(path, source, offset, &args, "array")?
            };
            Ok(MetadataFieldSchema {
                optional: false,
                kind: MetadataFieldKind::Array(Box::new(inner)),
            })
        }
        "dictionary" => {
            let inner = if args.is_empty() {
                MetadataFieldSchema {
                    optional: false,
                    kind: MetadataFieldKind::Any,
                }
            } else {
                parse_single_schema_arg(path, source, offset, &args, "dictionary")?
            };
            Ok(MetadataFieldSchema {
                optional: false,
                kind: MetadataFieldKind::Object(BTreeMap::from([("*".to_string(), inner)])),
            })
        }
        "object" => {
            let inner = parse_single_schema_expr(path, source, offset, &args, "object")?;
            let object = parse_schema_object(path, source, offset, inner)?;
            Ok(MetadataFieldSchema {
                optional: false,
                kind: MetadataFieldKind::Object(object.fields),
            })
        }
        "union" => {
            let mut options = Vec::new();
            for arg in args {
                let ast::Arg::Pos(expr) = arg else {
                    bail!(
                        "{}: union(...) only supports positional schema arguments",
                        metadata_location(path, source, offset)
                    );
                };
                options.push(parse_schema_field(path, source, offset, expr)?);
            }
            if options.is_empty() {
                bail!(
                    "{}: union(...) requires at least one schema argument",
                    metadata_location(path, source, offset)
                );
            }
            Ok(MetadataFieldSchema {
                optional: false,
                kind: MetadataFieldKind::Union(options),
            })
        }
        other => Ok(MetadataFieldSchema {
            optional: false,
            kind: schema_builtin_kind(other),
        }),
    }
}

fn parse_single_schema_arg<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    args: &[ast::Arg<'a>],
    name: &str,
) -> Result<MetadataFieldSchema> {
    let expr = parse_single_schema_expr(path, source, offset, args, name)?;
    parse_schema_field(path, source, offset, expr)
}

fn parse_single_schema_expr<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    args: &[ast::Arg<'a>],
    name: &str,
) -> Result<ast::Expr<'a>> {
    if args.len() != 1 {
        bail!(
            "{}: {name}(...) requires exactly one positional argument",
            metadata_location(path, source, offset)
        );
    }
    let ast::Arg::Pos(expr) = args[0] else {
        bail!(
            "{}: {name}(...) only supports a positional schema argument",
            metadata_location(path, source, offset)
        );
    };
    Ok(expr)
}

fn schema_builtin_kind(name: &str) -> MetadataFieldKind {
    let name = match name {
        "string" => "str",
        "boolean" => "bool",
        "integer" => "int",
        other => other,
    };
    if name == "any" {
        MetadataFieldKind::Any
    } else {
        MetadataFieldKind::Builtin(name.to_string())
    }
}

fn schema_call_name<'a>(call: ast::FuncCall<'a>) -> Option<&'a str> {
    match call.callee() {
        ast::Expr::Ident(ident) => Some(ident.as_str()),
        ast::Expr::FieldAccess(access) => Some(access.field().as_str()),
        _ => None,
    }
}

fn is_schema_function_call(call: ast::FuncCall, target: &str) -> bool {
    let ast::Expr::FieldAccess(access) = call.callee() else {
        return false;
    };
    if access.field().as_str() != "with" {
        return false;
    }
    matches!(access.target(), ast::Expr::Ident(ident) if ident.as_str() == target)
}

fn metadata_dict_items<'a>(
    path: &Path,
    source: &str,
    offset: usize,
    context: &str,
    expr: ast::Expr<'a>,
) -> Result<Vec<(String, ast::Expr<'a>)>> {
    let expr = match expr {
        ast::Expr::Parenthesized(value) => value.expr(),
        other => other,
    };
    let ast::Expr::Dict(dict) = expr else {
        bail!(
            "{}: `{context}` must be a dictionary",
            metadata_location(path, source, offset)
        );
    };
    let mut items = Vec::new();
    for item in dict.items() {
        match item {
            ast::DictItem::Named(named) => {
                items.push((named.name().as_str().to_string(), named.expr()));
            }
            ast::DictItem::Keyed(keyed) => {
                let ast::Expr::Str(key) = keyed.key() else {
                    bail!(
                        "{}: `{context}` dictionary keys must be identifiers or strings",
                        metadata_location(path, source, offset)
                    );
                };
                items.push((key.get().to_string(), keyed.expr()));
            }
            ast::DictItem::Spread(_) => {
                bail!(
                    "{}: `{context}` does not support spread expressions",
                    metadata_location(path, source, offset)
                );
            }
        }
    }
    Ok(items)
}

fn find_typst_metadata_directives(
    path: &Path,
    source: &str,
    parse_source: &str,
    base_offset: usize,
) -> Result<Vec<TypstMetadataDirective>> {
    let root = typst_syntax::parse(parse_source);
    let linked = LinkedNode::new(&root);
    let mut directives = Vec::new();
    for child in linked.children() {
        let Some(show) = child.get().cast::<ast::ShowRule>() else {
            continue;
        };
        let Some((name, call)) = metadata_directive_call(show) else {
            continue;
        };
        let mut range = child.range().start + base_offset..child.range().end + base_offset;
        if range.start > 0 && source.as_bytes().get(range.start - 1) == Some(&b'#') {
            range.start -= 1;
        }
        let meta = frontmatter_from_metadata_call(path, source, range.start, name, call)?;
        directives.push(TypstMetadataDirective { range, meta });
    }
    Ok(directives)
}

fn metadata_directive_call<'a>(show: ast::ShowRule<'a>) -> Option<(&'a str, ast::FuncCall<'a>)> {
    if show.selector().is_some() {
        return None;
    }
    let ast::Expr::FuncCall(call) = show.transform() else {
        return None;
    };
    let ast::Expr::FieldAccess(access) = call.callee() else {
        return None;
    };
    if access.field().as_str() != "with" {
        return None;
    }
    let ast::Expr::Ident(target) = access.target() else {
        return None;
    };
    match target.as_str() {
        "page" | "project" => Some((target.as_str(), call)),
        _ => None,
    }
}

fn frontmatter_from_metadata_call(
    path: &Path,
    source: &str,
    offset: usize,
    directive_name: &str,
    call: ast::FuncCall,
) -> Result<FrontMatter> {
    let mut meta = FrontMatter::default();
    let mut seen = BTreeSet::new();
    for arg in call.args().items() {
        let ast::Arg::Named(named) = arg else {
            bail!(
                "{}: `{}.with(...)` metadata only supports named arguments",
                metadata_location(path, source, offset),
                directive_name
            );
        };
        let name = named.name().as_str();
        if !seen.insert(name.to_string()) {
            bail!(
                "{}: duplicate metadata argument `{name}`",
                metadata_location(path, source, offset)
            );
        }
        apply_metadata_argument(path, source, offset, &mut meta, name, named.expr())?;
    }
    Ok(meta)
}

fn apply_metadata_argument(
    path: &Path,
    source: &str,
    offset: usize,
    meta: &mut FrontMatter,
    name: &str,
    expr: ast::Expr,
) -> Result<()> {
    match name {
        "title" => meta.title = metadata_optional_string(path, source, offset, name, expr)?,
        "description" => {
            meta.description = metadata_optional_string(path, source, offset, name, expr)?
        }
        "date" => meta.date = metadata_optional_string(path, source, offset, name, expr)?,
        "updated" => meta.updated = metadata_optional_string(path, source, offset, name, expr)?,
        "expires" => meta.expires = metadata_optional_string(path, source, offset, name, expr)?,
        "lang" => meta.lang = metadata_optional_string(path, source, offset, name, expr)?,
        "slug" => meta.slug = metadata_optional_string(path, source, offset, name, expr)?,
        "path" | "permalink" => {
            meta.permalink = metadata_optional_string(path, source, offset, name, expr)?
        }
        "template" => meta.template = metadata_optional_string(path, source, offset, name, expr)?,
        "section" => meta.section = metadata_optional_string(path, source, offset, name, expr)?,
        "excerpt" => meta.excerpt = metadata_optional_string(path, source, offset, name, expr)?,
        "sort_by" => meta.sort_by = metadata_optional_string(path, source, offset, name, expr)?,
        "draft" => meta.draft = metadata_bool(path, source, offset, name, expr)?.unwrap_or(false),
        "build_pdf" => meta.build_pdf = metadata_bool(path, source, offset, name, expr)?,
        "toc" => meta.toc = metadata_bool(path, source, offset, name, expr)?,
        "weight" => meta.weight = metadata_i64(path, source, offset, name, expr)?,
        "paginate_by" => meta.paginate_by = metadata_usize(path, source, offset, name, expr)?,
        "aliases" => meta.aliases = metadata_string_array(path, source, offset, name, expr)?,
        "tags" => meta.tags = metadata_string_array(path, source, offset, name, expr)?,
        "categories" => meta.categories = metadata_string_array(path, source, offset, name, expr)?,
        "extra" => bail!(
            "{}: metadata field `extra` is not supported; declare collection fields directly",
            metadata_location(path, source, offset)
        ),
        _ => {
            meta.fields.insert(
                name.to_string(),
                metadata_field(path, source, offset, name, expr)?,
            );
        }
    }
    Ok(())
}

fn metadata_field(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<MetadataField> {
    validate_metadata_field_name(path, name)?;
    let typst = expr.to_untyped().full_text().trim().to_string();
    if typst.is_empty() {
        bail!(
            "{}: metadata field `{name}` has an empty expression",
            metadata_location(path, source, offset)
        );
    }
    let value = metadata_toml_value(path, source, offset, name, expr)
        .ok()
        .flatten();
    Ok(MetadataField { typst, value })
}

fn metadata_optional_string(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<Option<String>> {
    match metadata_toml_value(path, source, offset, name, expr)? {
        None => Ok(None),
        Some(toml::Value::String(value)) => Ok(Some(value)),
        Some(value) => bail_metadata_type(path, source, offset, name, "a string or none", &value),
    }
}

fn metadata_bool(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<Option<bool>> {
    match metadata_toml_value(path, source, offset, name, expr)? {
        None => Ok(None),
        Some(toml::Value::Boolean(value)) => Ok(Some(value)),
        Some(value) => bail_metadata_type(path, source, offset, name, "a boolean or none", &value),
    }
}

fn metadata_i64(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<Option<i64>> {
    match metadata_toml_value(path, source, offset, name, expr)? {
        None => Ok(None),
        Some(toml::Value::Integer(value)) => Ok(Some(value)),
        Some(value) => bail_metadata_type(path, source, offset, name, "an integer or none", &value),
    }
}

fn metadata_usize(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<Option<usize>> {
    match metadata_i64(path, source, offset, name, expr)? {
        None => Ok(None),
        Some(value) if value >= 0 => Ok(Some(value as usize)),
        Some(_) => bail!(
            "{}: metadata field `{name}` must be a non-negative integer",
            metadata_location(path, source, offset)
        ),
    }
}

fn metadata_string_array(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<Vec<String>> {
    match metadata_toml_value(path, source, offset, name, expr)? {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(values)) => values
            .into_iter()
            .map(|value| match value {
                toml::Value::String(value) => Ok(value),
                other => {
                    bail_metadata_type(path, source, offset, name, "a tuple of strings", &other)
                }
            })
            .collect(),
        Some(value) => bail_metadata_type(
            path,
            source,
            offset,
            name,
            "a tuple of strings or none",
            &value,
        ),
    }
}

fn metadata_toml_value(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expr: ast::Expr,
) -> Result<Option<toml::Value>> {
    Ok(match expr {
        ast::Expr::None(_) => None,
        ast::Expr::Parenthesized(value) => {
            metadata_toml_value(path, source, offset, name, value.expr())?
        }
        ast::Expr::Str(value) => Some(toml::Value::String(value.get().to_string())),
        ast::Expr::Bool(value) => Some(toml::Value::Boolean(value.get())),
        ast::Expr::Int(value) => Some(toml::Value::Integer(value.get())),
        ast::Expr::Float(value) => Some(metadata_float_value(path, source, offset, name, value.get())?),
        ast::Expr::Unary(value) => metadata_unary_value(path, source, offset, name, value)?,
        ast::Expr::Array(value) => {
            let mut array = Vec::new();
            for item in value.items() {
                let ast::ArrayItem::Pos(expr) = item else {
                    bail!(
                        "{}: metadata field `{name}` does not support spread expressions in tuples",
                        metadata_location(path, source, offset)
                    );
                };
                match metadata_toml_value(path, source, offset, name, expr)? {
                    Some(value) => array.push(value),
                    None => bail!(
                        "{}: metadata field `{name}` cannot contain none inside a tuple",
                        metadata_location(path, source, offset)
                    ),
                }
            }
            Some(toml::Value::Array(array))
        }
        ast::Expr::Dict(value) => {
            let mut table = toml::Table::new();
            for item in value.items() {
                match item {
                    ast::DictItem::Named(named) => {
                        if let Some(value) =
                            metadata_toml_value(path, source, offset, name, named.expr())?
                        {
                            table.insert(named.name().as_str().to_string(), value);
                        }
                    }
                    ast::DictItem::Keyed(keyed) => {
                        let key = match keyed.key() {
                            ast::Expr::Str(value) => value.get().to_string(),
                            other => {
                                bail!(
                                    "{}: metadata field `{name}` only supports string dictionary keys, got `{}`",
                                    metadata_location(path, source, offset),
                                    concise_expr(other)
                                );
                            }
                        };
                        if let Some(value) =
                            metadata_toml_value(path, source, offset, name, keyed.expr())?
                        {
                            table.insert(key, value);
                        }
                    }
                    ast::DictItem::Spread(_) => {
                        bail!(
                            "{}: metadata field `{name}` does not support spread expressions in dictionaries",
                            metadata_location(path, source, offset)
                        );
                    }
                }
            }
            Some(toml::Value::Table(table))
        }
        unsupported => bail!(
            "{}: unsupported metadata expression for `{name}`: `{}`; use string, boolean, number, none, tuple, or dictionary literals",
            metadata_location(path, source, offset),
            concise_expr(unsupported)
        ),
    })
}

fn metadata_unary_value(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    value: ast::Unary,
) -> Result<Option<toml::Value>> {
    match (value.op(), value.expr()) {
        (ast::UnOp::Pos, ast::Expr::Int(value)) => Ok(Some(toml::Value::Integer(value.get()))),
        (ast::UnOp::Neg, ast::Expr::Int(value)) => Ok(Some(toml::Value::Integer(-value.get()))),
        (ast::UnOp::Pos, ast::Expr::Float(value)) => {
            Ok(Some(metadata_float_value(path, source, offset, name, value.get())?))
        }
        (ast::UnOp::Neg, ast::Expr::Float(value)) => {
            Ok(Some(metadata_float_value(path, source, offset, name, -value.get())?))
        }
        _ => bail!(
            "{}: unsupported metadata expression for `{name}`: `{}`; unary metadata values must be signed numbers",
            metadata_location(path, source, offset),
            concise_expr(ast::Expr::Unary(value))
        ),
    }
}

fn metadata_float_value(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    value: f64,
) -> Result<toml::Value> {
    if value.is_finite() {
        Ok(toml::Value::Float(value))
    } else {
        bail!(
            "{}: metadata field `{name}` must be a finite float",
            metadata_location(path, source, offset)
        );
    }
}

fn bail_metadata_type<T>(
    path: &Path,
    source: &str,
    offset: usize,
    name: &str,
    expected: &str,
    value: &toml::Value,
) -> Result<T> {
    bail!(
        "{}: metadata field `{name}` must be {expected}, got {}",
        metadata_location(path, source, offset),
        toml_value_kind(value)
    );
}

fn toml_value_kind(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "tuple",
        toml::Value::Table(_) => "dictionary",
    }
}

fn concise_expr(expr: ast::Expr) -> String {
    let text = expr.to_untyped().full_text().to_string();
    const MAX: usize = 80;
    if text.chars().count() <= MAX {
        text
    } else {
        let mut out = text.chars().take(MAX).collect::<String>();
        out.push_str("...");
        out
    }
}

fn metadata_location(path: &Path, source: &str, offset: usize) -> String {
    let (line, column) = line_column(source, offset);
    format!("{}:{line}:{column}", path.display())
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn remove_metadata_directive(raw: &str, range: std::ops::Range<usize>) -> String {
    let mut end = range.end;
    while raw[end..].starts_with('\n') {
        end += 1;
    }
    let mut body = String::with_capacity(raw.len().saturating_sub(end - range.start));
    body.push_str(&raw[..range.start]);
    body.push_str(&raw[end..]);
    body
}

fn is_content_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| name.starts_with('_') && name != "_index.typ")
        .unwrap_or(false)
}

fn page_address(
    cfg: &Config,
    rel: &Path,
    section: &str,
    meta: &FrontMatter,
) -> Result<PageAddress> {
    let mut no_ext = rel.to_path_buf();
    no_ext.set_extension("");
    let path = to_posix_path(&no_ext);
    let file_path = to_posix_path(rel);
    let filename = no_ext
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("index");
    let inferred_slug = if filename == "index" {
        no_ext
            .parent()
            .and_then(|p| p.file_name())
            .and_then(OsStr::to_str)
            .unwrap_or("index")
            .to_string()
    } else {
        filename.to_string()
    };
    let slug = meta.slug.clone().unwrap_or(inferred_slug);
    validate_slug(&slug)?;

    let url = if let Some(pattern) = meta.permalink.as_deref().or(cfg.permalink.as_deref()) {
        apply_permalink_pattern(
            pattern,
            section,
            &slug,
            &path,
            filename,
            meta.date.as_deref(),
        )?
    } else if meta.slug.is_some() {
        let clean = slug.trim_matches('/');
        if clean.is_empty() {
            "/".to_string()
        } else {
            format!("/{clean}/")
        }
    } else if path == "index" {
        "/".to_string()
    } else if path.ends_with("/index") {
        format!("/{}/", path.trim_end_matches("/index"))
    } else {
        format!("/{path}/")
    };
    let url = normalize_directory_url(&url)?;
    Ok(PageAddress {
        canonical_url: url.clone(),
        url,
        slug,
        path,
        file_path,
    })
}

fn apply_permalink_pattern(
    pattern: &str,
    section: &str,
    slug: &str,
    path: &str,
    filename: &str,
    date: Option<&str>,
) -> Result<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        bail!("permalink pattern must not be empty");
    }
    let (year, month, day) = date_parts(date);
    let mut out = pattern.to_string();
    for (key, value) in [
        ("section", section),
        ("slug", slug),
        ("path", path),
        ("filename", filename),
        ("year", year.as_deref().unwrap_or("")),
        ("month", month.as_deref().unwrap_or("")),
        ("day", day.as_deref().unwrap_or("")),
    ] {
        out = out.replace(&format!(":{key}"), value);
        out = out.replace(&format!("{{{key}}}"), value);
    }
    normalize_directory_url(&out)
}

fn date_parts(date: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(prefix) = simple_date_prefix(date) else {
        return (None, None, None);
    };
    let parts = prefix.split('-').map(str::to_string).collect::<Vec<_>>();
    if parts.len() == 3 {
        (
            Some(parts[0].clone()),
            Some(parts[1].clone()),
            Some(parts[2].clone()),
        )
    } else {
        (None, None, None)
    }
}

fn normalize_directory_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        bail!("URL must not be empty");
    }
    if trimmed.contains('\\') || trimmed.contains('?') || trimmed.contains('#') {
        bail!("unsafe URL path: {trimmed}");
    }
    let mut clean = trimmed.trim_matches('/').to_string();
    if clean.is_empty() {
        return Ok("/".to_string());
    }
    for seg in clean.split('/') {
        validate_url_segment(seg)?;
    }
    clean = clean
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!("/{clean}/"))
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.trim().is_empty() {
        bail!("slug must not be empty");
    }
    if slug.contains('/') {
        bail!("slug must be a single URL segment: {slug}");
    }
    validate_url_segment(slug)
}

fn validate_url_segment(seg: &str) -> Result<()> {
    if seg.is_empty()
        || seg == "."
        || seg == ".."
        || seg.contains('\\')
        || seg.contains('?')
        || seg.contains('#')
    {
        bail!("unsafe URL segment: {seg}");
    }
    Ok(())
}

fn infer_section(rel: &Path) -> String {
    let parent = rel.parent().unwrap_or_else(|| Path::new(""));
    if parent.as_os_str().is_empty() {
        return "pages".to_string();
    }
    to_posix_path(parent)
}

fn parent_section_name(section: &str) -> Option<String> {
    if section == "pages" {
        return None;
    }
    section
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
}

fn section_ancestors(section: &str) -> Vec<String> {
    if section == "pages" {
        return Vec::new();
    }
    let mut ancestors = Vec::new();
    let mut parts = Vec::new();
    for part in section.split('/') {
        if part.is_empty() {
            continue;
        }
        parts.push(part);
        let current = parts.join("/");
        if current != section {
            ancestors.push(current);
        }
    }
    ancestors
}

fn today_ymd_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

// Howard Hinnant's civil_from_days algorithm, adapted for UTC days since 1970-01-01.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m as u32, d as u32)
}

fn simple_date_prefix(date: Option<&str>) -> Option<String> {
    let date = date?.trim();
    if date.len() >= 10 {
        let prefix = &date[..10];
        let bytes = prefix.as_bytes();
        if bytes.get(4) == Some(&b'-')
            && bytes.get(7) == Some(&b'-')
            && prefix.chars().filter(|&c| c == '-').count() == 2
        {
            return Some(prefix.to_string());
        }
    }
    None
}

fn is_future_date(date: Option<&str>) -> bool {
    simple_date_prefix(date)
        .map(|d| d > today_ymd_utc())
        .unwrap_or(false)
}

fn is_expired_date(date: Option<&str>) -> bool {
    simple_date_prefix(date)
        .map(|d| d < today_ymd_utc())
        .unwrap_or(false)
}

fn default_title(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Untitled")
        .replace(['-', '_'], " ")
}

fn build_link_map(pages: &[Page]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for page in pages {
        let rel = page.rel.to_string_lossy().replace('\\', "/");
        let no_ext = rel.trim_end_matches(".typ").to_string();
        map.insert(format!("@/{rel}"), page.url.clone());
        map.insert(format!("@/{no_ext}"), page.url.clone());
        map.insert(rel, page.url.clone());
        map.insert(no_ext, page.url.clone());
        for alias in &page.meta.aliases {
            map.insert(alias.clone(), page.url.clone());
            map.insert(
                format!("@/{}", alias.trim_start_matches('/')),
                page.url.clone(),
            );
            if let Ok(alias_url) = alias_to_url(alias) {
                map.insert(alias_url.clone(), page.url.clone());
                map.insert(
                    format!("@/{}", alias_url.trim_matches('/')),
                    page.url.clone(),
                );
            }
        }
    }
    map
}

fn preprocess_body(
    body: &str,
    link_map: &BTreeMap<String, String>,
    make_toc: bool,
) -> (Vec<TocItem>, String, Vec<String>) {
    let mut broken = Vec::new();
    let expanded = expand_shortcodes(body);
    let resolved = resolve_internal_links(&expanded, link_map, &mut broken);
    if !make_toc {
        return (Vec::new(), resolved, broken);
    }
    let mut used = BTreeSet::new();
    let mut toc = Vec::new();
    let mut out = String::new();
    let mut in_raw_block = false;
    for line in resolved.lines() {
        if line.trim_start().starts_with("```") {
            in_raw_block = !in_raw_block;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !in_raw_block {
            if let Some((level, text)) = parse_heading(line) {
                let mut id = slugify(&text);
                if used.contains(&id) {
                    let base = id.clone();
                    let mut n = 2;
                    while used.contains(&format!("{base}-{n}")) {
                        n += 1;
                    }
                    id = format!("{base}-{n}");
                }
                used.insert(id.clone());
                toc.push(TocItem {
                    level,
                    id: id.clone(),
                    text,
                });
                out.push_str(&format!("#context {{ if target() == \"html\" {{ html.elem(\"a\", attrs: (id: {}, class: \"anchor\"))[] }} }}\n", typst_string(&id)));
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    (toc, out, broken)
}

fn expand_shortcodes(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_raw_block = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_raw_block = !in_raw_block;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_raw_block {
            out.push_str(line);
        } else {
            out.push_str(&expand_shortcodes_line(line));
        }
        out.push('\n');
    }
    out
}

fn expand_shortcodes_line(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("{{") {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        let after = &after[2..];
        if let Some(end) = after.find("}}") {
            let inner = after[..end].trim();
            if let Some(rendered) = render_shortcode(inner) {
                out.push_str(&rendered);
            } else {
                out.push_str("{{");
                out.push_str(inner);
                out.push_str("}}");
            }
            rest = &after[end + 2..];
        } else {
            out.push_str("{{");
            out.push_str(after);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn render_shortcode(inner: &str) -> Option<String> {
    let open = inner.find('(')?;
    let close = inner.rfind(')')?;
    if close <= open {
        return None;
    }
    let name = inner[..open].trim();
    let args = parse_shortcode_args(&inner[open + 1..close]);
    match name {
        "note" => {
            let text = args.get("text").or_else(|| args.get("0"))?.clone();
            Some(format!(
                "#context {{ let text = {}; if target() == \"html\" {{ html.elem(\"aside\", attrs: (class: \"note\"))[#text] }} else {{ block(inset: 8pt, stroke: luma(180))[#text] }} }}",
                typst_string(&text)
            ))
        }
        "figure" => {
            let src = args.get("src")?.clone();
            let alt = args.get("alt").cloned().unwrap_or_default();
            let caption = args.get("caption").cloned();
            let cap = match caption {
                Some(c) => format!("some({})", typst_string(&c)),
                None => "none".to_string(),
            };
            Some(format!(
                "#context {{ let src = {}; let alt = {}; let caption = {}; if target() == \"html\" {{ html.elem(\"figure\", attrs: (class: \"figure\"))[#html.elem(\"img\", attrs: (src: src, alt: alt))[] #if caption != none {{ html.elem(\"figcaption\")[#caption] }}] }} else {{ if caption != none {{ figure(image(src), caption: caption) }} else {{ image(src) }} }} }}",
                typst_string(&src), typst_string(&alt), cap
            ))
        }
        "youtube" => {
            let id = args.get("id")?.clone();
            let title = args
                .get("title")
                .cloned()
                .unwrap_or_else(|| "YouTube video".to_string());
            let src = format!("https://www.youtube-nocookie.com/embed/{id}");
            Some(format!(
                "#context {{ let src = {}; let title = {}; if target() == \"html\" {{ html.elem(\"div\", attrs: (class: \"embed embed-youtube\"))[#html.elem(\"iframe\", attrs: (src: src, title: title, loading: \"lazy\", allowfullscreen: \"\"))[]] }} else {{ link(src)[#title] }} }}",
                typst_string(&src), typst_string(&title)
            ))
        }
        _ => None,
    }
}

fn parse_shortcode_args(args: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (idx, part) in split_shortcode_args(args).into_iter().enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once('=') {
            map.insert(k.trim().to_string(), unquote(v.trim()));
        } else {
            map.insert(idx.to_string(), unquote(part));
        }
    }
    map
}

fn split_shortcode_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;
    for ch in args.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            cur.push(ch);
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            cur.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            cur.push(ch);
        } else if ch == ',' {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn unquote(s: &str) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len().saturating_sub(1)]
            .replace("\\\"", "\"")
            .replace("\\'", "'")
    } else {
        trimmed.to_string()
    }
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('=') {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '=').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = trimmed[level..].trim_start();
    if rest.is_empty() || rest.starts_with('=') {
        return None;
    }
    Some((level, strip_inline_markup(rest)))
}

fn strip_inline_markup(s: &str) -> String {
    s.replace('`', "")
        .replace('*', "")
        .replace('_', "")
        .trim()
        .to_string()
}

fn resolve_internal_links(
    body: &str,
    link_map: &BTreeMap<String, String>,
    broken: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_raw_block = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_raw_block = !in_raw_block;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_raw_block {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(&resolve_internal_links_line(line, link_map, broken));
        out.push('\n');
    }
    out
}

fn resolve_internal_links_line(
    line: &str,
    link_map: &BTreeMap<String, String>,
    broken: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(line.len());
    let chars = line.chars().collect::<Vec<_>>();
    let mut i = 0usize;
    let mut in_inline_raw = false;
    while i < chars.len() {
        if chars[i] == '`' {
            in_inline_raw = !in_inline_raw;
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if !in_inline_raw && chars[i] == '@' && i + 1 < chars.len() && chars[i + 1] == '/' {
            let start = i;
            let mut j = i + 2;
            while j < chars.len() && !is_link_delim(chars[j]) {
                j += 1;
            }
            let raw_key = chars[start..j].iter().collect::<String>();
            let (key, fragment) = split_fragment(&raw_key);
            if let Some(url) = link_map.get(key) {
                out.push_str(url);
                if let Some(fragment) = fragment {
                    out.push('#');
                    out.push_str(fragment);
                }
            } else {
                broken.push(raw_key.clone());
                out.push_str(&raw_key);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn split_fragment(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once('#') {
        Some((key, fragment)) => (key, Some(fragment)),
        None => (raw, None),
    }
}

fn is_link_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '}' | '<' | '>')
}

fn section_url(section: &str) -> String {
    if section == "pages" {
        return "/".to_string();
    }
    let parts = section
        .split('/')
        .filter(|p| !p.is_empty())
        .map(slugify)
        .collect::<Vec<_>>();
    format!("/{}/", parts.join("/"))
}

fn make_generated_pages(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
    section_meta: &BTreeMap<String, FrontMatter>,
) -> Vec<GeneratedPage> {
    let mut generated = Vec::new();
    let page_urls = pages
        .iter()
        .map(|page| page.url.as_str())
        .collect::<BTreeSet<_>>();
    let mut sections: BTreeMap<String, Vec<PageSummary>> = BTreeMap::new();
    for page in pages {
        sections
            .entry(page.section.clone())
            .or_default()
            .push(page.clone());
    }
    for (section, mut items) in sections {
        if section == "pages" {
            continue;
        }
        let meta = section_meta.get(&section);
        let sort_by = meta
            .and_then(|m| m.sort_by.as_deref())
            .unwrap_or("date_desc");
        sort_page_summaries(&mut items, sort_by);
        let title = meta
            .and_then(|m| m.title.clone())
            .unwrap_or_else(|| section.clone());
        let description = meta
            .and_then(|m| m.description.clone())
            .or_else(|| Some(format!("Pages in {section}")));
        let paginate_by = meta.and_then(|m| m.paginate_by).or(cfg.paginate_by);
        let url = section_url(&section);
        if page_urls.contains(url.as_str()) {
            continue;
        }
        push_paginated(
            &mut generated,
            out_root,
            "section",
            title,
            description,
            url,
            items,
            paginate_by,
        );
    }

    for taxonomy in &cfg.taxonomies {
        let mut terms: BTreeMap<String, Vec<PageSummary>> = BTreeMap::new();
        for page in pages {
            for value in taxonomy_values(page, &taxonomy.name) {
                terms.entry(value).or_default().push(page.clone());
            }
        }
        let mut all_terms = Vec::new();
        for (term, mut items) in terms {
            sort_page_summaries(&mut items, "date_desc");
            let term_url = format!("/{}/{}/", taxonomy.slug, slugify(&term));
            all_terms.push(PageSummary {
                kind: "term".to_string(),
                title: term.clone(),
                url: term_url.clone(),
                canonical_url: term_url.clone(),
                slug: slugify(&term),
                path: term_url.trim_matches('/').to_string(),
                file_path: String::new(),
                description: Some(format!("{} page(s)", items.len())),
                date: None,
                updated: None,
                weight: None,
                section: taxonomy.name.clone(),
                parent_section: None,
                ancestors: Vec::new(),
                tags: Vec::new(),
                categories: Vec::new(),
                aliases: Vec::new(),
                source: String::new(),
                excerpt: None,
                toc: Vec::new(),
                fields: BTreeMap::new(),
            });
            push_paginated(
                &mut generated,
                out_root,
                &taxonomy.name,
                format!("{}: {}", taxonomy.name, term),
                None,
                term_url,
                items,
                cfg.paginate_by,
            );
        }
        all_terms.sort_by(|a, b| a.title.cmp(&b.title));
        push_paginated(
            &mut generated,
            out_root,
            &format!("{}-index", taxonomy.name),
            taxonomy.name.clone(),
            Some(format!("All {}", taxonomy.name)),
            format!("/{}/", taxonomy.slug),
            all_terms,
            cfg.paginate_by,
        );
    }
    generated
}

fn sort_page_summaries(items: &mut [PageSummary], sort_by: &str) {
    match sort_by {
        "weight" | "weight_asc" => items.sort_by(|a, b| {
            a.weight
                .unwrap_or(0)
                .cmp(&b.weight.unwrap_or(0))
                .then(a.url.cmp(&b.url))
        }),
        "weight_desc" => items.sort_by(|a, b| {
            b.weight
                .unwrap_or(0)
                .cmp(&a.weight.unwrap_or(0))
                .then(a.url.cmp(&b.url))
        }),
        "date" | "date_desc" => items.sort_by(|a, b| {
            b.date
                .cmp(&a.date)
                .then(a.weight.unwrap_or(0).cmp(&b.weight.unwrap_or(0)))
                .then(a.url.cmp(&b.url))
        }),
        "date_asc" => items.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then(a.weight.unwrap_or(0).cmp(&b.weight.unwrap_or(0)))
                .then(a.url.cmp(&b.url))
        }),
        "updated" | "updated_desc" => items.sort_by(|a, b| {
            b.updated
                .cmp(&a.updated)
                .then(b.date.cmp(&a.date))
                .then(a.url.cmp(&b.url))
        }),
        "updated_asc" => items.sort_by(|a, b| {
            a.updated
                .cmp(&b.updated)
                .then(a.date.cmp(&b.date))
                .then(a.url.cmp(&b.url))
        }),
        "title" | "title_asc" => {
            items.sort_by(|a, b| a.title.cmp(&b.title).then(a.url.cmp(&b.url)))
        }
        "title_desc" => items.sort_by(|a, b| b.title.cmp(&a.title).then(a.url.cmp(&b.url))),
        "url" | "url_asc" => items.sort_by(|a, b| a.url.cmp(&b.url)),
        "url_desc" => items.sort_by(|a, b| b.url.cmp(&a.url)),
        _ => items.sort_by(|a, b| {
            b.date
                .cmp(&a.date)
                .then(a.weight.unwrap_or(0).cmp(&b.weight.unwrap_or(0)))
                .then(a.url.cmp(&b.url))
        }),
    }
}

fn push_paginated(
    generated: &mut Vec<GeneratedPage>,
    out_root: &Path,
    kind: &str,
    title: String,
    description: Option<String>,
    base_url: String,
    items: Vec<PageSummary>,
    paginate_by: Option<usize>,
) {
    let page_size = paginate_by.filter(|n| *n > 0).unwrap_or(items.len().max(1));
    let total_pages = ((items.len().max(1) + page_size - 1) / page_size).max(1);
    for idx in 0..total_pages {
        let page_number = idx + 1;
        let start = idx * page_size;
        let end = usize::min(start + page_size, items.len());
        let page_items = if start < items.len() {
            items[start..end].to_vec()
        } else {
            Vec::new()
        };
        let url = paginated_url(&base_url, page_number);
        let prev_url = if page_number > 1 {
            Some(paginated_url(&base_url, page_number - 1))
        } else {
            None
        };
        let next_url = if page_number < total_pages {
            Some(paginated_url(&base_url, page_number + 1))
        } else {
            None
        };
        generated.push(GeneratedPage {
            kind: kind.to_string(),
            title: if page_number == 1 {
                title.clone()
            } else {
                format!("{} - page {}", title, page_number)
            },
            description: description.clone(),
            output_html: output_path_for_url(out_root, &url)
                .unwrap_or_else(|_| out_root.join("invalid").join("index.html")),
            url,
            items: page_items,
            page_number,
            total_pages,
            prev_title: prev_url.as_ref().map(|_| "Previous".to_string()),
            prev_url,
            next_title: next_url.as_ref().map(|_| "Next".to_string()),
            next_url,
        });
    }
}

fn paginated_url(base_url: &str, page_number: usize) -> String {
    if page_number <= 1 {
        base_url.to_string()
    } else {
        format!("{}/page/{}/", base_url.trim_end_matches('/'), page_number)
    }
}

fn taxonomy_values(page: &PageSummary, name: &str) -> Vec<String> {
    match name {
        "tags" => return page.tags.clone(),
        "categories" => return page.categories.clone(),
        _ => {}
    }
    match page.fields.get(name).and_then(|field| field.value.as_ref()) {
        Some(toml::Value::String(s)) => vec![s.clone()],
        Some(toml::Value::Array(xs)) => xs
            .iter()
            .filter_map(|x| x.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn page_outputs(page: &Page, pdf: bool) -> Vec<PathBuf> {
    let mut outputs = vec![page.output_html.clone()];
    if pdf {
        outputs.push(page.output_pdf.clone());
    }
    outputs
}

fn outputs_to_strings(outputs: &[PathBuf]) -> Vec<String> {
    outputs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

struct CacheDecision {
    hit: bool,
    reasons: Vec<String>,
}

fn cache_decision(
    cache: &BuildCache,
    key: &str,
    hash: &str,
    outputs: &[PathBuf],
    force: bool,
) -> CacheDecision {
    if force {
        return CacheDecision {
            hit: false,
            reasons: vec!["forced rebuild".to_string()],
        };
    }
    let Some(entry) = cache.entries.get(key) else {
        return CacheDecision {
            hit: false,
            reasons: vec!["no cache entry".to_string()],
        };
    };
    let mut reasons = Vec::new();
    if entry.hash != hash {
        reasons.push("hash changed".to_string());
    }
    for output in outputs {
        match fs::symlink_metadata(output) {
            Ok(meta) if meta.file_type().is_file() => {}
            Ok(_) => reasons.push(format!("unsafe output path {}", output.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                reasons.push(format!("missing output {}", output.display()));
            }
            Err(err) => {
                reasons.push(format!(
                    "failed to inspect output {}: {err}",
                    output.display()
                ));
            }
        }
    }
    if reasons.is_empty() {
        CacheDecision {
            hit: true,
            reasons: vec!["cache hit".to_string()],
        }
    } else {
        CacheDecision {
            hit: false,
            reasons,
        }
    }
}

fn print_explain(action: &str, label: &str, reasons: &[String]) {
    let action = match action {
        "skip" => term::green("skip"),
        "rebuild" => term::yellow("rebuild"),
        other => term::cyan(other),
    };
    println!("  {} {}", action, label);
    for reason in reasons {
        println!("    {} {}", term::dim("reason"), reason);
    }
}

fn format_duration(duration: Duration) -> String {
    let ms = duration.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{:.2}s", duration.as_secs_f64())
    }
}

fn print_build_summary(
    stats: &BuildStats,
    compile_jobs: usize,
    out_root: &Path,
    elapsed: Duration,
) {
    let status = if stats.failed == 0 {
        term::green("✓")
    } else {
        term::red("✗")
    };
    println!(
        "{} {} {} {} {} {} {} {} {}",
        status,
        term::bold("build"),
        term::dim("pages"),
        term::green(stats.built.to_string()),
        term::dim("generated"),
        term::green(stats.generated.to_string()),
        term::dim("skipped"),
        term::yellow(stats.skipped.to_string()),
        term::dim(format!(
            "drafts {} future {} expired {} failed {} jobs {}",
            stats.drafts, stats.future, stats.expired, stats.failed, compile_jobs
        )),
    );
    println!("  {} {}", term::dim("out"), out_root.display());
    println!("  {} {}", term::dim("time"), format_duration(elapsed));
}

fn print_profile(reports: &[CompileReport], compile_wall: Duration) {
    if reports.is_empty() {
        println!("{} no Typst compile jobs", term::magenta("profile"));
        return;
    }
    let cpu_sum = reports
        .iter()
        .fold(Duration::ZERO, |acc, r| acc + r.duration);
    println!(
        "{} compile wall={} cpu-sum={} jobs={}",
        term::magenta("profile"),
        format_duration(compile_wall),
        format_duration(cpu_sum),
        reports.len()
    );
    let mut slow = reports.to_vec();
    slow.sort_by_key(|r| std::cmp::Reverse(r.duration));
    println!("  {}", term::dim("slowest"));
    for report in slow.iter().take(10) {
        let kind = if report.generated {
            "generated"
        } else {
            "page"
        };
        println!(
            "  {} {} {} {}",
            term::yellow(format_duration(report.duration)),
            term::dim(kind),
            report.label,
            term::dim(format!("{} output(s)", report.output_count))
        );
    }
}

fn effective_jobs(cli_jobs: Option<usize>, cfg_jobs: usize, job_count: usize) -> usize {
    if job_count == 0 {
        return 0;
    }
    let requested = cli_jobs.unwrap_or(cfg_jobs);
    let jobs = if requested == 0 {
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    } else {
        requested
    };
    jobs.max(1).min(job_count)
}

fn run_compile_jobs(
    cfg: &Config,
    root: &Path,
    jobs: Vec<CompileJob>,
    concurrency: usize,
) -> Vec<(CompileJob, Result<CompileReport>)> {
    if jobs.is_empty() {
        return Vec::new();
    }
    if concurrency <= 1 {
        return jobs
            .into_iter()
            .map(|job| {
                let result = run_compile_job(cfg, root, &job);
                (job, result)
            })
            .collect();
    }

    let (job_tx, job_rx) = mpsc::channel::<CompileJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (out_tx, out_rx) = mpsc::channel::<(CompileJob, Result<CompileReport>)>();

    for _ in 0..concurrency {
        let job_rx = Arc::clone(&job_rx);
        let out_tx = out_tx.clone();
        let cfg = cfg.clone();
        let root = root.to_path_buf();
        thread::spawn(move || loop {
            let next = { job_rx.lock().expect("compile job receiver poisoned").recv() };
            let Ok(job) = next else {
                break;
            };
            let result = run_compile_job(&cfg, &root, &job);
            let _ = out_tx.send((job, result));
        });
    }
    drop(out_tx);

    for job in jobs {
        let _ = job_tx.send(job);
    }
    drop(job_tx);

    out_rx.into_iter().collect()
}

fn run_compile_job(cfg: &Config, root: &Path, job: &CompileJob) -> Result<CompileReport> {
    let started = Instant::now();
    let inputs = [("typage_current", job.current_json.as_str())];
    compile_typst(
        cfg,
        root,
        &job.input,
        &job.html_output,
        "html",
        &job.html_features,
        job.label.clone(),
        &inputs,
    )?;
    if let Some(pdf_output) = &job.pdf_output {
        compile_typst(
            cfg,
            root,
            &job.input,
            pdf_output,
            "pdf",
            "",
            format!("{} [pdf]", job.label),
            &inputs,
        )?;
    }
    Ok(CompileReport {
        label: job.label.clone(),
        duration: started.elapsed(),
        output_count: job.outputs.len(),
        generated: job.generated,
    })
}

fn nav_json(title: &Option<String>, url: &Option<String>) -> serde_json::Value {
    match (title, url) {
        (Some(title), Some(url)) => json!({ "title": title, "url": url }),
        _ => serde_json::Value::Null,
    }
}

fn page_current_json(page: &Page) -> Result<String> {
    let mut value = json!({
        "kind": "page",
        "title": &page.title,
        "description": &page.meta.description,
        "excerpt": &page.meta.excerpt,
        "date": &page.meta.date,
        "updated": &page.meta.updated,
        "weight": page.meta.weight,
        "lang": page.meta.lang.as_deref().unwrap_or("en"),
        "url": &page.url,
        "canonical_url": &page.canonical_url,
        "current_url": &page.url,
        "slug": &page.slug,
        "path": &page.path,
        "file_path": &page.file_path,
        "source": page.rel.to_string_lossy().replace('\\', "/"),
        "section": &page.section,
        "parent_section": &page.parent_section,
        "ancestors": &page.ancestors,
        "tags": &page.meta.tags,
        "categories": &page.meta.categories,
        "aliases": &page.meta.aliases,
        "toc": &page.toc,
        "prev": nav_json(&page.prev_title, &page.prev_url),
        "next": nav_json(&page.next_title, &page.next_url),
    });
    if let Some(object) = value.as_object_mut() {
        for (name, field) in &page.meta.fields {
            if let Some(value) = &field.value {
                object.insert(name.clone(), serde_json::to_value(value)?);
            }
        }
    }
    Ok(serde_json::to_string(&value)?)
}

fn generated_current_json(gen: &GeneratedPage, cfg: &Config) -> Result<String> {
    Ok(serde_json::to_string(&json!({
        "kind": &gen.kind,
        "title": &gen.title,
        "description": &gen.description,
        "date": serde_json::Value::Null,
        "updated": serde_json::Value::Null,
        "weight": serde_json::Value::Null,
        "lang": &cfg.lang,
        "url": &gen.url,
        "canonical_url": &gen.url,
        "current_url": &gen.url,
        "slug": serde_json::Value::Null,
        "path": &gen.url,
        "file_path": serde_json::Value::Null,
        "source": serde_json::Value::Null,
        "section": &gen.kind,
        "parent_section": serde_json::Value::Null,
        "ancestors": [],
        "tags": [],
        "categories": [],
        "aliases": [],
        "toc": [],
        "prev": nav_json(&gen.prev_title, &gen.prev_url),
        "next": nav_json(&gen.next_title, &gen.next_url),
        "items": &gen.items,
        "page_number": gen.page_number,
        "total_pages": gen.total_pages,
    }))?)
}

fn read_cache(path: &Path) -> Result<BuildCache> {
    if !path.exists() {
        return Ok(BuildCache::default());
    }
    let raw = fs::read_to_string(path).unwrap_or_default();
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn write_cache(path: &Path, cache: &BuildCache) -> Result<()> {
    write_if_changed(path, &serde_json::to_string_pretty(cache)?)
}

fn cfg_hash(cfg: &Config) -> String {
    serde_json::to_string(cfg).unwrap_or_default()
}

fn cached_template_hash(
    cache: &mut BTreeMap<String, String>,
    templates_root: &Path,
    template: &str,
) -> Result<String> {
    if let Some(hash) = cache.get(template) {
        return Ok(hash.clone());
    }
    let hash = template_hash(templates_root, template)?;
    cache.insert(template.to_string(), hash.clone());
    Ok(hash)
}

fn template_hash(templates_root: &Path, template: &str) -> Result<String> {
    let path = templates_root.join(template);
    file_with_deps_hash(&path)
}

fn page_wrapper_path(wrappers_root: &Path, page: &Page) -> PathBuf {
    let mut path = wrappers_root.join(&page.rel);
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("page")
        .to_string();
    path.set_file_name(format!("{stem}.wrapper.typ"));
    path
}

fn page_wrapper_source(
    root: &Path,
    cfg: &Config,
    _data_path: &Path,
    page: &Page,
    wrapper_path: &Path,
    collections: &ContentCollections,
) -> Result<String> {
    let wrapper_dir = wrapper_path.parent().unwrap_or(root);
    fs::create_dir_all(wrapper_dir)?;
    let source_dir = page.source.parent().unwrap_or(root);
    let template_path = templates_root(root, cfg).join(&page.template);
    let template_rel = relative_path(wrapper_dir, &template_path)?;
    let body = rewrite_local_dependency_paths(&page.processed_body, source_dir, wrapper_dir)?;
    let current = page_dict(page);
    let validation = page_metadata_validation_typ(page, collections);
    Ok(format!(
        "#import {}: render\n#import \"@local/typage:{TYPAGE_VERSION}\" as typage\n\n#let __typage_current = {}\n{}#show: render.with(site: typage.site, page: __typage_current, pages: typage.pages, taxonomies: typage.taxonomies)\n\n{}\n",
        typst_string(&to_posix_path(&template_rel)),
        current,
        validation,
        body
    ))
}

fn rewrite_local_dependency_paths(
    body: &str,
    source_dir: &Path,
    target_dir: &Path,
) -> Result<String> {
    let mut out = String::new();
    let mut raw_fence_len = None;
    for line in body.lines() {
        if let Some(fence_len) = line_raw_fence_len(line) {
            match raw_fence_len {
                Some(open_len) if fence_len >= open_len => raw_fence_len = None,
                None => raw_fence_len = Some(fence_len),
                _ => {}
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if raw_fence_len.is_some() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(&rewrite_dependency_line(line, source_dir, target_dir)?);
        out.push('\n');
    }
    Ok(out)
}

fn rewrite_dependency_line(line: &str, source_dir: &Path, target_dir: &Path) -> Result<String> {
    let mut current = line.to_string();
    for marker in ["#import", "#include", "read(", "image("] {
        current = rewrite_dependency_marker(&current, marker, source_dir, target_dir)?;
    }
    Ok(current)
}

fn rewrite_dependency_marker(
    line: &str,
    marker: &str,
    source_dir: &Path,
    target_dir: &Path,
) -> Result<String> {
    let mut out = String::new();
    let mut rest = line;
    while let Some(marker_pos) = rest.find(marker) {
        let (before, after_marker_start) = rest.split_at(marker_pos + marker.len());
        out.push_str(before);
        let after_marker = after_marker_start;
        let Some(first_quote_rel) = after_marker.find('"') else {
            rest = after_marker;
            continue;
        };
        out.push_str(&after_marker[..first_quote_rel + 1]);
        let quoted = &after_marker[first_quote_rel + 1..];
        let Some(second_quote_rel) = quoted.find('"') else {
            rest = quoted;
            continue;
        };
        let path = &quoted[..second_quote_rel];
        if is_local_typst_path(path) {
            let abs = source_dir.join(path);
            if abs.exists() {
                let rel = relative_path(target_dir, &abs)?;
                out.push_str(&to_posix_path(&rel));
            } else {
                out.push_str(path);
            }
        } else {
            out.push_str(path);
        }
        out.push('"');
        rest = &quoted[second_quote_rel + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn generated_wrapper_source(
    root: &Path,
    cfg: &Config,
    _data_path: &Path,
    gen: &GeneratedPage,
) -> Result<String> {
    let wrapper_dir = root.join(&cfg.cache_dir).join("generated");
    fs::create_dir_all(&wrapper_dir)?;
    let template_path = templates_root(root, cfg).join(&cfg.list_template);
    let template_rel = relative_path(&wrapper_dir, &template_path)?;
    let _ = gen;
    Ok(format!(
        "#import {}: render\n#import \"@local/typage:{TYPAGE_VERSION}\" as typage\n\n#show: render.with(site: typage.site, page: typage.current, pages: typage.pages, taxonomies: typage.taxonomies)\n\n",
        typst_string(&to_posix_path(&template_rel)),
    ))
}

fn page_dict(page: &Page) -> String {
    format!(
        "(kind: \"page\", title: {}, description: {}, excerpt: {}, date: {}, updated: {}, weight: {}, lang: {}, url: {}, canonical_url: {}, current_url: {}, slug: {}, path: {}, file_path: {}, source: {}, section: {}, parent_section: {}, ancestors: {}, tags: {}, categories: {}, aliases: {}, toc: {}, prev: {}, next: {}{})",
        typst_string(&page.title),
        typst_opt_string(&page.meta.description),
        typst_opt_string(&page.meta.excerpt),
        typst_opt_string(&page.meta.date),
        typst_opt_string(&page.meta.updated),
        page.meta.weight.map(|w| w.to_string()).unwrap_or_else(|| "none".to_string()),
        typst_string(page.meta.lang.as_deref().unwrap_or("en")),
        typst_string(&page.url),
        typst_string(&page.canonical_url),
        typst_string(&page.url),
        typst_string(&page.slug),
        typst_string(&page.path),
        typst_string(&page.file_path),
        typst_string(&page.rel.to_string_lossy().replace('\\', "/")),
        typst_string(&page.section),
        typst_opt_string(&page.parent_section),
        typst_array_str(&page.ancestors),
        typst_array_str(&page.meta.tags),
        typst_array_str(&page.meta.categories),
        typst_array_str(&page.meta.aliases),
        toc_array(&page.toc),
        nav_dict(&page.prev_title, &page.prev_url),
        nav_dict(&page.next_title, &page.next_url),
        metadata_fields_typst(&page.meta.fields),
    )
}

fn page_metadata_validation_typ(page: &Page, collections: &ContentCollections) -> String {
    let Some(schema) = collections.schema_for(&page.section) else {
        return String::new();
    };
    let mut out = String::new();
    for (idx, (name, field_schema)) in schema.fields.iter().enumerate() {
        let var = format!("__typage_meta_{idx}");
        out.push_str(&format!(
            "#let {var} = __typage_current.at({}, default: none)\n",
            typst_string(name)
        ));
        if !field_schema.optional {
            out.push_str(&format!(
                "#if {var} == none {{ panic({}) }}\n",
                typst_string(&format!(
                    "{}: missing required metadata field `{name}`",
                    page.rel.display()
                ))
            ));
        }
        out.push_str(&metadata_schema_validation_typ(
            &var,
            name,
            field_schema,
            &format!("{}.{name}", page.rel.display()),
        ));
    }
    out
}

fn metadata_schema_validation_typ(
    expr: &str,
    label: &str,
    schema: &MetadataFieldSchema,
    message_path: &str,
) -> String {
    let mut out = String::new();
    match &schema.kind {
        MetadataFieldKind::Any => {}
        MetadataFieldKind::Builtin(name) => {
            let condition = typst_type_condition(expr, name);
            out.push_str(&format!(
                "#if {expr} != none and not ({condition}) {{ panic({}) }}\n",
                typst_string(&format!(
                    "{message_path}: metadata field `{label}` must be {name}"
                ))
            ));
        }
        MetadataFieldKind::Array(inner) => {
            out.push_str(&format!(
                "#if {expr} != none and type({expr}) != array {{ panic({}) }}\n",
                typst_string(&format!(
                    "{message_path}: metadata field `{label}` must be array"
                ))
            ));
            out.push_str(&format!(
                "#if {expr} != none and type({expr}) == array {{\n"
            ));
            out.push_str(&format!("  for __typage_item in {expr} {{\n"));
            let nested = metadata_schema_validation_code(
                "__typage_item",
                label,
                inner,
                &format!("{message_path}[]"),
                "    ",
            );
            out.push_str(&nested);
            out.push_str("  }\n}\n");
        }
        MetadataFieldKind::Object(fields) if fields.contains_key("*") => {
            out.push_str(&format!(
                "#if {expr} != none and type({expr}) != dictionary {{ panic({}) }}\n",
                typst_string(&format!(
                    "{message_path}: metadata field `{label}` must be dictionary"
                ))
            ));
        }
        MetadataFieldKind::Object(fields) => {
            out.push_str(&format!(
                "#if {expr} != none and type({expr}) != dictionary {{ panic({}) }}\n",
                typst_string(&format!(
                    "{message_path}: metadata field `{label}` must be dictionary"
                ))
            ));
            out.push_str(&format!(
                "#if {expr} != none and type({expr}) == dictionary {{\n"
            ));
            for (idx, (key, inner)) in fields.iter().enumerate() {
                let var = format!("__typage_obj_{idx}");
                out.push_str(&format!(
                    "  let {var} = {expr}.at({}, default: none)\n",
                    typst_string(key)
                ));
                if !inner.optional {
                    out.push_str(&format!(
                        "  if {var} == none {{ panic({}) }}\n",
                        typst_string(&format!(
                            "{message_path}: missing required metadata field `{label}.{key}`"
                        ))
                    ));
                }
                out.push_str(&metadata_schema_validation_code(
                    &var,
                    &format!("{label}.{key}"),
                    inner,
                    message_path,
                    "  ",
                ));
            }
            out.push_str("}\n");
        }
        MetadataFieldKind::Union(options) => {
            let condition = options
                .iter()
                .map(|schema| metadata_schema_match_condition(expr, schema))
                .collect::<Vec<_>>()
                .join(" or ");
            out.push_str(&format!(
                "#if {expr} != none and not ({condition}) {{ panic({}) }}\n",
                typst_string(&format!(
                    "{message_path}: metadata field `{label}` does not match union schema"
                ))
            ));
        }
    }
    out
}

fn metadata_schema_validation_code(
    expr: &str,
    label: &str,
    schema: &MetadataFieldSchema,
    message_path: &str,
    indent: &str,
) -> String {
    metadata_schema_validation_typ(expr, label, schema, message_path)
        .lines()
        .map(|line| format!("{indent}{}\n", line.trim_start_matches('#')))
        .collect::<String>()
}

fn metadata_schema_match_condition(expr: &str, schema: &MetadataFieldSchema) -> String {
    if schema.optional {
        return format!(
            "{expr} == none or ({})",
            metadata_schema_match_condition_inner(expr, schema)
        );
    }
    metadata_schema_match_condition_inner(expr, schema)
}

fn metadata_schema_match_condition_inner(expr: &str, schema: &MetadataFieldSchema) -> String {
    match &schema.kind {
        MetadataFieldKind::Any => "true".to_string(),
        MetadataFieldKind::Builtin(name) => typst_type_condition(expr, name),
        MetadataFieldKind::Array(_) => format!("type({expr}) == array"),
        MetadataFieldKind::Object(_) => format!("type({expr}) == dictionary"),
        MetadataFieldKind::Union(options) => options
            .iter()
            .map(|schema| metadata_schema_match_condition(expr, schema))
            .collect::<Vec<_>>()
            .join(" or "),
    }
}

fn typst_type_condition(expr: &str, name: &str) -> String {
    match name {
        "date" | "datetime" => format!("type({expr}) == datetime or type({expr}) == str"),
        "url" => format!("type({expr}) == str"),
        "number" => {
            format!("type({expr}) == int or type({expr}) == float or type({expr}) == decimal")
        }
        "none" => format!("{expr} == none"),
        "auto" => format!("{expr} == auto"),
        other => format!("type({expr}) == {other}"),
    }
}

fn nav_dict(title: &Option<String>, url: &Option<String>) -> String {
    match (title, url) {
        (Some(title), Some(url)) => format!(
            "(title: {}, url: {})",
            typst_string(title),
            typst_string(url)
        ),
        _ => "none".to_string(),
    }
}

fn generated_page_dict(gen: &GeneratedPage, cfg: &Config) -> String {
    let items = typst_tuple(gen.items.iter().map(summary_dict).collect::<Vec<_>>());
    format!(
        "(kind: {}, title: {}, description: {}, date: none, updated: none, weight: none, lang: {}, url: {}, canonical_url: {}, current_url: {}, slug: none, path: {}, file_path: none, source: none, section: {}, parent_section: none, ancestors: (), tags: (), categories: (), aliases: (), toc: (), prev: {}, next: {}, items: {}, page_number: {}, total_pages: {})",
        typst_string(&gen.kind),
        typst_string(&gen.title),
        typst_opt_string(&gen.description),
        typst_string(&cfg.lang),
        typst_string(&gen.url),
        typst_string(&gen.url),
        typst_string(&gen.url),
        typst_string(gen.url.trim_matches('/')),
        typst_string(&gen.kind),
        nav_dict(&gen.prev_title, &gen.prev_url),
        nav_dict(&gen.next_title, &gen.next_url),
        items,
        gen.page_number,
        gen.total_pages,
    )
}

fn summary_dict(page: &PageSummary) -> String {
    format!(
        "(kind: {}, title: {}, url: {}, canonical_url: {}, slug: {}, path: {}, file_path: {}, description: {}, date: {}, updated: {}, weight: {}, section: {}, parent_section: {}, ancestors: {}, tags: {}, categories: {}, aliases: {}, source: {}, excerpt: {}, toc: {}{})",
        typst_string(&page.kind),
        typst_string(&page.title),
        typst_string(&page.url),
        typst_string(&page.canonical_url),
        typst_string(&page.slug),
        typst_string(&page.path),
        typst_string(&page.file_path),
        typst_opt_string(&page.description),
        typst_opt_string(&page.date),
        typst_opt_string(&page.updated),
        page.weight.map(|w| w.to_string()).unwrap_or_else(|| "none".to_string()),
        typst_string(&page.section),
        typst_opt_string(&page.parent_section),
        typst_array_str(&page.ancestors),
        typst_array_str(&page.tags),
        typst_array_str(&page.categories),
        typst_array_str(&page.aliases),
        typst_string(&page.source),
        typst_opt_string(&page.excerpt),
        toc_array(&page.toc),
        metadata_fields_typst(&page.fields),
    )
}

fn metadata_fields_typst(fields: &BTreeMap<String, MetadataField>) -> String {
    if fields.is_empty() {
        return String::new();
    }
    let fields = fields
        .iter()
        .map(|(name, field)| format!("{}: {}", typst_ident(name), field.typst))
        .collect::<Vec<_>>()
        .join(", ");
    format!(", {fields}")
}

fn typst_ident(key: &str) -> String {
    let mut out = String::new();
    for (i, ch) in key.chars().enumerate() {
        if ch == '_' || ch.is_ascii_alphabetic() || (i > 0 && ch.is_ascii_digit()) {
            out.push(ch);
        } else if ch.is_alphabetic() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty()
        || out
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(true)
    {
        out.insert(0, '_');
    }
    out
}

fn typst_toml_table(map: &BTreeMap<String, toml::Value>) -> String {
    if map.is_empty() {
        return "(:)".to_string();
    }
    let fields = map
        .iter()
        .map(|(k, v)| format!("{}: {}", typst_ident(k), typst_value(v)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("({fields})")
}

fn typst_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => typst_string(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(dt) => typst_string(&dt.to_string()),
        toml::Value::Array(xs) => typst_tuple(xs.iter().map(typst_value).collect()),
        toml::Value::Table(t) => {
            let fields = t
                .iter()
                .map(|(k, v)| format!("{}: {}", typst_ident(k), typst_value(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({fields})")
        }
    }
}

fn toc_array(toc: &[TocItem]) -> String {
    let items = toc
        .iter()
        .map(|item| {
            format!(
                "(level: {}, id: {}, text: {})",
                item.level,
                typst_string(&item.id),
                typst_string(&item.text)
            )
        })
        .collect::<Vec<_>>();
    typst_tuple(items)
}

fn site_data_typ(
    cfg: &Config,
    pages: &[PageSummary],
    generated: &[GeneratedPage],
    section_meta: &BTreeMap<String, FrontMatter>,
) -> String {
    let pages_typ = typst_tuple(pages.iter().map(summary_dict).collect::<Vec<_>>());
    let generated_typ = typst_tuple(
        generated
            .iter()
            .map(|g| {
                format!(
                    "(kind: {}, title: {}, url: {}, items: {}, page_number: {}, total_pages: {})",
                    typst_string(&g.kind),
                    typst_string(&g.title),
                    typst_string(&g.url),
                    g.items.len(),
                    g.page_number,
                    g.total_pages
                )
            })
            .collect::<Vec<_>>(),
    );
    let sections_typ = sections_dict_typ(pages, section_meta);
    let taxonomies = cfg
        .taxonomies
        .iter()
        .map(|t| {
            format!(
                "{}: (name: {}, slug: {})",
                typst_ident(&t.name),
                typst_string(&t.name),
                typst_string(&t.slug)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "// Generated by typage. Do not edit.\n#let site = (title: {}, base_url: {}, lang: {}, extra: {})\n#let pages = {}\n#let generated = {}\n#let sections = {}\n#let taxonomies = ({})\n",
        typst_string(&cfg.title),
        typst_string(&cfg.base_url),
        typst_string(&cfg.lang),
        typst_toml_table(&cfg.extra),
        pages_typ,
        generated_typ,
        sections_typ,
        taxonomies,
    )
}

fn sections_dict_typ(
    pages: &[PageSummary],
    section_meta: &BTreeMap<String, FrontMatter>,
) -> String {
    let mut names = BTreeSet::<String>::new();
    names.insert("pages".to_string());
    for page in pages {
        names.insert(page.section.clone());
        for ancestor in &page.ancestors {
            names.insert(ancestor.clone());
        }
        if let Some(parent) = &page.parent_section {
            names.insert(parent.clone());
        }
    }
    for name in section_meta.keys() {
        names.insert(name.clone());
    }

    let items = names.iter().map(|name| {
        let meta = section_meta.get(name);
        let pages_in_section = pages.iter().filter(|p| &p.section == name).map(summary_dict).collect::<Vec<_>>();
        let children = names.iter()
            .filter(|candidate| parent_section_name(candidate).as_deref() == Some(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let parent = parent_section_name(name);
        let ancestors = section_ancestors(name);
        format!(
            "(name: {}, title: {}, description: {}, path: {}, parent: {}, ancestors: {}, children: {}, pages: {}, sort_by: {}, weight: {}{})",
            typst_string(name),
            typst_string(&meta.and_then(|m| m.title.clone()).unwrap_or_else(|| name.clone())),
            typst_opt_string(&meta.and_then(|m| m.description.clone())),
            typst_string(&section_url(name)),
            typst_opt_string(&parent),
            typst_array_str(&ancestors),
            typst_array_str(&children),
            typst_tuple(pages_in_section),
            typst_opt_string(&meta.and_then(|m| m.sort_by.clone())),
            meta.and_then(|m| m.weight).map(|w| w.to_string()).unwrap_or_else(|| "none".to_string()),
            meta.map(|m| metadata_fields_typst(&m.fields)).unwrap_or_default(),
        )
    }).collect::<Vec<_>>();
    typst_tuple(items)
}

fn typage_package_root(root: &Path, cfg: &Config) -> PathBuf {
    root.join(&cfg.cache_dir)
        .join(format!("packages/local/typage/{TYPAGE_VERSION}"))
}

fn typage_theme_package_root(root: &Path, cfg: &Config) -> PathBuf {
    root.join(&cfg.cache_dir)
        .join(format!("packages/local/typage-theme/{TYPAGE_VERSION}"))
}

fn write_typage_package_data(root: &Path, cfg: &Config, site_data: &str) -> Result<()> {
    let pkg_root = typage_package_root(root, cfg);
    fs::create_dir_all(&pkg_root)?;
    let manifest = format!(
        r#"[package]
name = "typage"
version = "{TYPAGE_VERSION}"
entrypoint = "lib.typ"
authors = ["typage"]
license = "MIT"
description = "Virtual site data package generated by typage."
"#
    );
    write_if_changed(&pkg_root.join("typst.toml"), &manifest)?;
    let lib = r#"#import "data.typ": site, pages, generated, sections, taxonomies

#let current = json(bytes(sys.inputs.at("typage_current", default: "{}")))

#let url(path) = if site.base_url == "" { path } else { site.base_url + path }
#let asset = url

#let is-current(page) = page.url == current.url

#let page-by-url(target) = {
  let found = pages.filter(page => page.url == target)
  if found.len() == 0 { none } else { found.first() }
}

#let section(name) = {
  let found = sections.filter(sec => sec.name == name)
  if found.len() == 0 { (name: name, title: name, description: none, path: "/" + name + "/", parent: none, ancestors: (), children: (), pages: (), sort_by: none, weight: none) } else { found.first() }
}

#let children(item) = {
  let name = if type(item) == str { item } else { item.name }
  section(name).children.map(child => section(child))
}

#let ancestors(item) = {
  let name = if type(item) == str { item } else if item.at("section", default: none) != none { item.section } else { item.name }
  section(name).ancestors.map(parent => section(parent))
}

#let siblings(page) = pages.filter(other => other.section == page.section and other.url != page.url)

#let taxonomy-url(name, term) = {
  let tax = taxonomies.at(name, default: (slug: name))
  "/" + tax.slug + "/" + str(term).replace(" ", "-") + "/"
}
"#;
    write_if_changed(&pkg_root.join("lib.typ"), lib)?;
    write_if_changed(&pkg_root.join("data.typ"), site_data)?;
    remove_dir_if_exists(
        &root
            .join(&cfg.cache_dir)
            .join("packages/local/typssg/0.1.0"),
    )?;
    Ok(())
}

fn write_theme_package_data(root: &Path, cfg: &Config) -> Result<()> {
    let pkg_root = typage_theme_package_root(root, cfg);
    remove_dir_if_exists(&pkg_root)?;
    fs::create_dir_all(&pkg_root)?;
    let manifest = format!(
        r#"[package]
name = "typage-theme"
version = "{TYPAGE_VERSION}"
entrypoint = "lib.typ"
authors = ["typage theme"]
license = "MIT"
description = "Virtual active theme component package generated by typage."
"#
    );
    write_if_changed(&pkg_root.join("typst.toml"), &manifest)?;

    let active_theme_meta = cfg
        .theme
        .as_deref()
        .and_then(|theme| read_theme_toml(&root.join("themes").join(theme)).ok())
        .unwrap_or_else(|| ThemeToml {
            name: Some("project".to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            description: Some("Project-local components".to_string()),
            min_typage: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..ThemeToml::default()
        });
    write_if_changed(
        &pkg_root.join("metadata.typ"),
        &theme_metadata_typ(&active_theme_meta),
    )?;

    let components_root = templates_root(root, cfg).join("components");
    if components_root.join("lib.typ").exists() {
        copy_dir(&components_root, &pkg_root)?;
    } else {
        let fallback = r#"// Generated fallback theme package.
// Define templates/components/lib.typ in your project or active theme to export components.
"#;
        write_if_changed(&pkg_root.join("lib.typ"), fallback)?;
    }

    remove_dir_if_exists(
        &root
            .join(&cfg.cache_dir)
            .join("packages/local/typssg-theme/0.1.0"),
    )?;
    Ok(())
}

fn compile_typst(
    cfg: &Config,
    root: &Path,
    input: &Path,
    output: &Path,
    format: &str,
    features: &str,
    label: String,
    inputs: &[(&str, &str)],
) -> Result<Duration> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    prepare_output_file(output)?;
    let mut cmd = Command::new(&cfg.typst);
    cmd.current_dir(root).arg("compile").arg("--root").arg(root);
    let package_path = root.join(&cfg.cache_dir).join("packages");
    if package_path.exists() {
        cmd.arg("--package-path").arg(&package_path);
    }
    for (key, value) in inputs {
        cmd.arg("--input").arg(format!("{key}={value}"));
    }
    if !features.is_empty() {
        cmd.arg("--features").arg(features);
    }
    cmd.arg("--format").arg(format).arg(input).arg(output);
    let rendered_cmd = format!("{:?}", cmd);
    let started = Instant::now();
    let out = cmd
        .output()
        .with_context(|| format!("failed to start typst for {label}"))?;
    let elapsed = started.elapsed();
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "typst failed for {label}\ncommand: {rendered_cmd}\ninput: {}\noutput: {}\nstatus: {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            input.display(),
            output.display(),
            out.status,
            stdout,
            stderr,
        );
    }
    Ok(elapsed)
}

fn prepare_output_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                fs::remove_file(path).with_context(|| {
                    format!("failed to remove stale output symlink {}", path.display())
                })?;
            } else if !file_type.is_file() {
                bail!("refusing to overwrite non-file output {}", path.display());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect output path {}", path.display()));
        }
    }
    Ok(())
}

fn bundle_source(
    root: &Path,
    cfg: &Config,
    data_path: &Path,
    pages: &[Page],
    generated: &[GeneratedPage],
    collections: &ContentCollections,
) -> Result<String> {
    let bundle_dir = root.join(&cfg.cache_dir);
    let data_rel = relative_path(&bundle_dir, data_path)?;
    let mut out = String::new();
    out.push_str("// Generated by typage. Experimental Typst bundle export.\n");
    out.push_str(&format!(
        "#import {}: site, pages, taxonomies\n",
        typst_string(&to_posix_path(&data_rel))
    ));
    for page in pages {
        let template_path = templates_root(root, cfg).join(&page.template);
        let template_rel = relative_path(&bundle_dir, &template_path)?;
        let out_path = page.url.trim_start_matches('/').trim_end_matches('/');
        let html_path = if out_path.is_empty() {
            "index.html".to_string()
        } else {
            format!("{out_path}/index.html")
        };
        out.push_str(&format!("#document({})[\n", typst_string(&html_path)));
        out.push_str(&format!(
            "  #import {}: render\n",
            typst_string(&to_posix_path(&template_rel))
        ));
        out.push_str(&format!("  #let current = {}\n", page_dict(page)));
        out.push_str("  #let __typage_current = current\n");
        out.push_str(
            &page_metadata_validation_typ(page, collections)
                .lines()
                .map(|line| format!("  {line}\n"))
                .collect::<String>(),
        );
        out.push_str("  #let asset = path => if site.base_url == \"\" { path } else { site.base_url + path }\n");
        out.push_str("  #show: render.with(site: site, page: current, pages: pages, taxonomies: taxonomies)\n");
        let source_dir = page.source.parent().unwrap_or(root);
        let body = rewrite_local_dependency_paths(&page.processed_body, source_dir, &bundle_dir)?;
        out.push_str(&body);
        out.push_str("\n]\n");
    }
    for gen in generated {
        let template_path = templates_root(root, cfg).join(&cfg.list_template);
        let template_rel = relative_path(&bundle_dir, &template_path)?;
        let out_path = gen.url.trim_start_matches('/').trim_end_matches('/');
        let html_path = if out_path.is_empty() {
            "index.html".to_string()
        } else {
            format!("{out_path}/index.html")
        };
        out.push_str(&format!("#document({})[\n", typst_string(&html_path)));
        out.push_str(&format!(
            "  #import {}: render\n",
            typst_string(&to_posix_path(&template_rel))
        ));
        out.push_str(&format!(
            "  #let current = {}\n",
            generated_page_dict(gen, cfg)
        ));
        out.push_str("  #let asset = path => if site.base_url == \"\" { path } else { site.base_url + path }\n");
        out.push_str("  #show: render.with(site: site, page: current, pages: pages, taxonomies: taxonomies)\n");
        out.push_str("]\n");
    }
    for static_root in static_roots(root, cfg) {
        if static_root.exists() {
            for entry in walkdir::WalkDir::new(&static_root) {
                let entry = entry?;
                if entry.file_type().is_file() {
                    let src = entry.path();
                    let rel = src
                        .strip_prefix(&static_root)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    let src_rel = relative_path(&bundle_dir, src)?;
                    out.push_str(&format!(
                        "#asset({}, read({}, encoding: none))\n",
                        typst_string(&rel),
                        typst_string(&to_posix_path(&src_rel))
                    ));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, FeedConfig};
    use std::path::Path;

    #[test]
    fn parses_typst_versions() {
        assert_eq!(
            parse_typst_version("typst 0.15.0 (unknown commit)"),
            Some((0, 15, 0))
        );
        assert_eq!(parse_typst_version("typst v0.16.1-dev"), Some((0, 16, 1)));
        assert_eq!(parse_typst_version("typst unknown"), None);
    }

    fn page_summary(title: &str, url: &str, section: &str, date: &str) -> PageSummary {
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
        let got =
            resolve_internal_links_line("#link(\"@/missing.typ\")[missing]", &map, &mut broken);
        assert_eq!(got, "#link(\"@/missing.typ\")[missing]");
        assert_eq!(broken, vec!["@/missing.typ".to_string()]);
    }

    #[test]
    fn rewrites_local_dependency_paths_for_isolated_wrappers() {
        let tmp = std::env::temp_dir().join(format!("typage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let content = tmp.join("content");
        let wrappers = tmp.join(".typage/wrappers");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&wrappers).unwrap();
        std::fs::write(content.join("dep.typ"), "#let x = 1").unwrap();
        let got =
            rewrite_local_dependency_paths("#import \"dep.typ\": x", &content, &wrappers).unwrap();
        assert!(got.contains("../../content/dep.typ") || got.contains("../content/dep.typ"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rejects_unsafe_aliases() {
        assert!(alias_to_url("old/hello").is_ok());
        assert!(alias_to_url("../escape").is_err());
        assert!(alias_to_url("").is_err());
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
    fn paginated_urls_are_directory_urls() {
        assert_eq!(paginated_url("/posts/", 1), "/posts/");
        assert_eq!(paginated_url("/posts/", 2), "/posts/page/2/");
    }

    #[test]
    fn section_index_page_suppresses_generated_section_page() {
        let cfg = Config::default();
        let out =
            std::env::temp_dir().join(format!("typage-section-index-test-{}", std::process::id()));
        let pages = vec![
            PageSummary {
                kind: "page".to_string(),
                title: "Awards".to_string(),
                url: "/awards/".to_string(),
                canonical_url: "/awards/".to_string(),
                slug: "awards".to_string(),
                path: "awards/index".to_string(),
                file_path: "awards/index.typ".to_string(),
                description: None,
                date: None,
                updated: None,
                weight: None,
                section: "awards".to_string(),
                parent_section: None,
                ancestors: Vec::new(),
                tags: Vec::new(),
                categories: Vec::new(),
                aliases: Vec::new(),
                source: "awards/index.typ".to_string(),
                excerpt: None,
                toc: Vec::new(),
                fields: BTreeMap::new(),
            },
            PageSummary {
                kind: "page".to_string(),
                title: "Child".to_string(),
                url: "/awards/child/".to_string(),
                canonical_url: "/awards/child/".to_string(),
                slug: "child".to_string(),
                path: "awards/child".to_string(),
                file_path: "awards/child.typ".to_string(),
                description: None,
                date: None,
                updated: None,
                weight: None,
                section: "awards".to_string(),
                parent_section: None,
                ancestors: Vec::new(),
                tags: Vec::new(),
                categories: Vec::new(),
                aliases: Vec::new(),
                source: "awards/child.typ".to_string(),
                excerpt: None,
                toc: Vec::new(),
                fields: BTreeMap::new(),
            },
        ];
        let generated = make_generated_pages(&cfg, &out, &pages, &BTreeMap::new());
        assert!(!generated.iter().any(|page| page.url == "/awards/"));
    }

    #[test]
    fn rejects_dangerous_directory_policy() {
        let root = Path::new("/tmp/site");
        let mut cfg = Config::default();
        cfg.out_dir = PathBuf::from("public");
        cfg.static_dir = PathBuf::from("public");
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

    #[cfg(unix)]
    #[test]
    fn cache_decision_rebuilds_unsafe_output_symlink() {
        use std::os::unix::fs::symlink;

        let tmp =
            std::env::temp_dir().join(format!("typage-cache-link-test-{}", std::process::id()));
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

        let tmp = std::env::temp_dir().join(format!(
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

        prepare_output_file(&output).unwrap();
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
        let tmp = std::env::temp_dir().join(format!("typage-pkg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let cfg = Config::default();
        write_typage_package_data(&tmp, &cfg, &site_data_typ(&cfg, &[], &[], &BTreeMap::new()))
            .unwrap();
        let pkg = tmp.join(format!(".typage/packages/local/typage/{TYPAGE_VERSION}"));
        let lib = std::fs::read_to_string(pkg.join("lib.typ")).unwrap();
        assert!(lib.contains("sys.inputs.at"));
        assert!(lib.contains("typage_current"));
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
        let tmp =
            std::env::temp_dir().join(format!("typage-theme-pkg-test-{}", std::process::id()));
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
    fn theme_metadata_validates_declared_components() {
        let tmp =
            std::env::temp_dir().join(format!("typage-theme-check-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let theme = tmp.join("themes/default");
        std::fs::create_dir_all(theme.join("templates/components")).unwrap();
        std::fs::create_dir_all(theme.join("static")).unwrap();
        std::fs::write(theme.join("theme.toml"), default_theme_toml("default")).unwrap();
        std::fs::write(
            theme.join("templates/components/lib.typ"),
            include_str!("../templates/components/lib.typ"),
        )
        .unwrap();
        let report = inspect_theme(&theme, Some("default"));
        assert!(report.ok(), "{:?}", report.errors);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn theme_check_reports_missing_declared_component() {
        let tmp =
            std::env::temp_dir().join(format!("typage-theme-missing-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let theme = tmp.join("themes/bad");
        std::fs::create_dir_all(theme.join("templates/components")).unwrap();
        std::fs::create_dir_all(theme.join("static")).unwrap();
        std::fs::write(
            theme.join("theme.toml"),
            format!(
                "name = \"bad\"\nversion = \"0.1.0\"\nmin_typage = \"{TYPAGE_VERSION}\"\n[components]\nnote = true\n"
            ),
        )
        .unwrap();
        std::fs::write(
            theme.join("templates/components/lib.typ"),
            "#let card(body) = body\n",
        )
        .unwrap();
        let report = inspect_theme(&theme, Some("bad"));
        assert!(!report.ok());
        assert!(report.errors.iter().any(|e| e.contains("note")));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn expands_builtin_shortcodes() {
        let got = expand_shortcodes("before {{ note(text=\"hello\") }} after");
        assert!(got.contains("html.elem(\"aside\""));
        assert!(got.contains("hello"));
    }

    #[test]
    fn search_plain_text_ignores_code() {
        let got = plain_text_for_search("== Title\n`code` text #strong[ok]");
        assert!(got.contains("Title"));
        assert!(got.contains("text"));
        assert!(!got.contains("code"));
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
    fn rss_date_formats_simple_dates() {
        assert_eq!(rss_date("2026-06-23"), "23 Jun 2026 00:00:00 +0000");
        assert_eq!(atom_date("2026-06-23"), "2026-06-23T00:00:00Z");
    }

    #[test]
    fn feeds_are_written_without_base_url() {
        let tmp = std::env::temp_dir().join(format!("typage-feed-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mut cfg = Config::default();
        cfg.base_url = String::new();
        let pages = vec![page_summary("Hello", "/hello/", "posts", "2026-06-23")];

        write_configured_feeds(&cfg, &tmp, &pages).unwrap();
        write_atom_feed(&cfg, &tmp, &pages).unwrap();

        let rss = std::fs::read_to_string(tmp.join("feed.xml")).unwrap();
        let atom = std::fs::read_to_string(tmp.join("atom.xml")).unwrap();
        assert!(rss.contains("<link>/</link>"));
        assert!(rss.contains("<link>/hello/</link>"));
        assert!(atom.contains("<link href=\"/\"/>"));
        assert!(atom.contains("<link href=\"/hello/\"/>"));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn configured_rss_feeds_filter_sections_and_paths() {
        let tmp = std::env::temp_dir().join(format!(
            "typage-configured-feed-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mut cfg = Config::default();
        cfg.base_url = String::new();
        cfg.feed_path = "rss.xml".to_string();
        cfg.feed_sections = vec!["blog".to_string()];
        cfg.feed_limit = 0;
        cfg.feeds = vec![FeedConfig {
            path: "projects/rss.xml".to_string(),
            title: Some("Projects".to_string()),
            description: Some("Project feed".to_string()),
            link: "/projects/".to_string(),
            section: Some("projects".to_string()),
            limit: 0,
            ..FeedConfig::default()
        }];
        let pages = vec![
            page_summary("Blog", "/blog/post/", "blog", "2026-06-23"),
            page_summary("Project", "/projects/tool/", "projects", "2026-06-22"),
            page_summary("Favorite", "/favorites/art/", "favorites", "2026-06-21"),
        ];

        write_configured_feeds(&cfg, &tmp, &pages).unwrap();

        let rss = std::fs::read_to_string(tmp.join("rss.xml")).unwrap();
        let projects = std::fs::read_to_string(tmp.join("projects/rss.xml")).unwrap();
        assert!(rss.contains("<title>Blog</title>"));
        assert!(!rss.contains("<title>Project</title>"));
        assert!(!rss.contains("<title>Favorite</title>"));
        assert!(projects.contains("<title>Project</title>"));
        assert!(!projects.contains("<title>Blog</title>"));
        assert!(!projects.contains("<title>Favorite</title>"));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn search_tokens_are_normalized_and_deduped() {
        let mut page = Page {
            source: PathBuf::from("content/posts/hello.typ"),
            rel: PathBuf::from("posts/hello.typ"),
            body: String::new(),
            processed_body: String::new(),
            meta: FrontMatter::default(),
            title: "Hello Rust".to_string(),
            url: "/posts/hello/".to_string(),
            canonical_url: "/posts/hello/".to_string(),
            slug: "hello".to_string(),
            path: "posts/hello".to_string(),
            file_path: "posts/hello.typ".to_string(),
            output_html: PathBuf::new(),
            output_pdf: PathBuf::new(),
            template: "base.typ".to_string(),
            section: "posts".to_string(),
            parent_section: None,
            ancestors: Vec::new(),
            toc: Vec::new(),
            hash: String::new(),
            prev_title: None,
            prev_url: None,
            next_title: None,
            next_url: None,
        };
        page.meta.tags = vec!["Typst".to_string(), "Rust".to_string()];
        let tokens = search_tokens(&page, "Rust rust HTML", "Typage");
        assert!(tokens.contains(&"rust".to_string()));
        assert!(tokens.contains(&"typst".to_string()));
        assert_eq!(tokens.iter().filter(|t| *t == "rust").count(), 1);
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
        assert_eq!(
            fm.fields
                .get("authors")
                .and_then(|field| field.value.as_ref())
                .and_then(|value| value.as_array())
                .and_then(|values| values.first())
                .and_then(|value| value.as_str()),
            Some("Eito")
        );
        assert_eq!(
            fm.fields.get("authors").map(|field| field.typst.as_str()),
            Some("(\"Eito\",)")
        );
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
        assert!(err.contains("`[extra]` is no longer supported"));
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
            meta.fields
                .get("languages")
                .and_then(|field| field.value.as_ref())
                .and_then(|value| value.as_array())
                .and_then(|values| values.first())
                .and_then(|value| value.as_str()),
            Some("Typst")
        );
        assert_eq!(
            meta.fields
                .get("links")
                .and_then(|field| field.value.as_ref())
                .and_then(|value| value.as_array())
                .and_then(|values| values.first())
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("label"))
                .and_then(|value| value.as_str()),
            Some("GitHub")
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
        let raw = r#"#show: page.with(title: upper("Hello"))

Body
"#;
        let err = split_metadata(Path::new("content/demo.typ"), raw)
            .unwrap_err()
            .to_string();
        assert!(err.contains("content/demo.typ:1:1"));
        assert!(err.contains("unsupported metadata expression"));
        assert!(err.contains("upper(\"Hello\")"));
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
            MetadataField {
                typst: "\"guide\"".to_string(),
                value: Some(toml::Value::String("guide".to_string())),
            },
        );
        let data = site_data_typ(&cfg, &[page], &[], &BTreeMap::new());
        assert!(data.contains("#let sections"));
        assert!(data.contains("posts/tutorials"));
        assert!(data.contains("parent:"));
        assert!(data.contains("ancestors:"));
    }

    #[test]
    fn search_tokenizer_emits_cjk_ngrams() {
        let cfg = SearchConfig {
            mode: "auto".to_string(),
            ngram: 2,
            ..SearchConfig::default()
        };
        let tokens = tokenize_search_text("\u{65e5}\u{672c}\u{8a9e}\u{691c}\u{7d22} Typst", &cfg);
        assert!(tokens.contains(&"\u{65e5}\u{672c}".to_string()));
        assert!(tokens.contains(&"\u{672c}\u{8a9e}".to_string()));
        assert!(tokens.contains(&"\u{8a9e}\u{691c}".to_string()));
        assert!(tokens.contains(&"\u{691c}\u{7d22}".to_string()));
        assert!(tokens.contains(&"typst".to_string()));
    }

    #[test]
    fn search_config_accepts_bool_and_table() {
        let bool_cfg: Config = toml::from_str("search = false").unwrap();
        assert!(!bool_cfg.search.enabled);
        let table_cfg: Config = toml::from_str(
            "[search]\nenabled = true\nmode = \"ngram\"\nngram = 3\ncompact = true\n",
        )
        .unwrap();
        assert!(table_cfg.search.enabled);
        assert_eq!(table_cfg.search.mode, "ngram");
        assert_eq!(table_cfg.search.ngram, 3);
        assert!(table_cfg.search.compact);
        let cjk_cfg: Config =
            toml::from_str("[search]\nenabled = true\nmode = \"cjk\"\nngram = 2\n").unwrap();
        assert_eq!(cjk_cfg.search.mode, "cjk");
    }

    #[test]
    fn permalink_policy_expands_placeholders() {
        let mut cfg = Config::default();
        cfg.permalink = Some("/docs/:year/:month/:slug/".to_string());
        let mut fm = FrontMatter::default();
        fm.title = Some("Intro".to_string());
        fm.date = Some("2026-06-24".to_string());
        fm.slug = Some("getting-started".to_string());
        let addr = page_address(&cfg, Path::new("posts/intro.typ"), "posts", &fm).unwrap();
        assert_eq!(addr.url, "/docs/2026/06/getting-started/");
        assert_eq!(addr.slug, "getting-started");
        assert_eq!(addr.path, "posts/intro");
        assert_eq!(addr.file_path, "posts/intro.typ");
    }

    #[test]
    fn page_permalink_overrides_global_policy() {
        let mut cfg = Config::default();
        cfg.permalink = Some("/:section/:slug/".to_string());
        let mut fm = FrontMatter::default();
        fm.slug = Some("v0-23".to_string());
        fm.permalink = Some("/releases/:slug/".to_string());
        let addr = page_address(&cfg, Path::new("posts/release.typ"), "posts", &fm).unwrap();
        assert_eq!(addr.url, "/releases/v0-23/");
    }

    #[test]
    fn unsafe_slug_and_permalink_are_rejected() {
        let cfg = Config::default();
        let mut fm = FrontMatter::default();
        fm.slug = Some("../escape".to_string());
        assert!(page_address(&cfg, Path::new("posts/hello.typ"), "posts", &fm).is_err());
        let mut fm = FrontMatter::default();
        fm.permalink = Some("/../escape/".to_string());
        assert!(page_address(&cfg, Path::new("posts/hello.typ"), "posts", &fm).is_err());
    }
}
