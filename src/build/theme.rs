//! Theme scaffolding, inspection, and active theme paths.
use super::{toml_string, typst_ident, typst_toml_table, write_if_missing, TYPAGE_VERSION};
use crate::config::{load_config, Config};
use crate::term;
use crate::util::{normalize_path, typst_opt_string};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct ThemeToml {
    pub(super) name: Option<String>,
    pub(super) version: Option<String>,
    pub(super) description: Option<String>,
    pub(super) min_typage: Option<String>,
    pub(super) components: BTreeMap<String, bool>,
    pub(super) extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone)]
pub(super) struct ThemeReport {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) meta: Option<ThemeToml>,
    pub(super) errors: Vec<String>,
    pub(super) warnings: Vec<String>,
}

impl ThemeReport {
    pub(super) fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn new_theme(root: PathBuf, name: String) -> Result<()> {
    let root = normalize_path(&root)?;
    validate_theme_name(&name)?;
    let theme_root = root.join("themes").join(&name);
    crate::util::create_dir_all_checked(theme_root.join("templates/components"))?;
    crate::util::create_dir_all_checked(theme_root.join("static"))?;
    write_if_missing(theme_root.join("theme.toml"), &default_theme_toml(&name))?;
    write_if_missing(
        theme_root.join("templates/base.typ"),
        include_str!("../../templates/base.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/list.typ"),
        include_str!("../../templates/list.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/print.typ"),
        include_str!("../../templates/print.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/helpers.typ"),
        include_str!("../../templates/helpers.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/layout.typ"),
        include_str!("../../templates/components/layout.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/lib.typ"),
        include_str!("../../templates/components/lib.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/callout.typ"),
        include_str!("../../templates/components/callout.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/card.typ"),
        include_str!("../../templates/components/card.typ"),
    )?;
    write_if_missing(
        theme_root.join("templates/components/media.typ"),
        include_str!("../../templates/components/media.typ"),
    )?;
    write_if_missing(
        theme_root.join("static/style.css"),
        include_str!("../../static/style.css"),
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

pub(super) fn read_theme_toml(theme_root: &Path) -> Result<ThemeToml> {
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

/// Collects manifest and component diagnostics without executing theme code.
/// Export checks are source-level checks, not a substitute for Typst compilation.
pub(super) fn inspect_theme(theme_root: &Path, expected_name: Option<&str>) -> ThemeReport {
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
        if verbose && !meta.extra.is_empty() {
            println!("extra:");
            for (key, value) in &meta.extra {
                println!("  {key}: {:?}", value);
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

pub(super) fn theme_metadata_typ(meta: &ThemeToml) -> String {
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

pub(super) fn validate_theme_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("theme name must not be empty");
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        bail!("invalid theme name: {name}");
    }
    Ok(())
}

pub(super) fn templates_root(root: &Path, cfg: &Config) -> PathBuf {
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        let themed = root.join("themes").join(theme).join("templates");
        if themed.exists() {
            return themed;
        }
    }
    root.join(&cfg.templates_dir)
}

/// Returns assets in overlay order: active theme first, project overrides last.
pub(super) fn static_roots(root: &Path, cfg: &Config) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        roots.push(root.join("themes").join(theme).join("static"));
    }
    roots.push(root.join(&cfg.static_dir));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_metadata_validates_declared_components() {
        let tmp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("typage-theme-check-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let theme = tmp.join("themes/default");
        std::fs::create_dir_all(theme.join("templates/components")).unwrap();
        std::fs::create_dir_all(theme.join("static")).unwrap();
        std::fs::write(theme.join("theme.toml"), default_theme_toml("default")).unwrap();
        std::fs::write(
            theme.join("templates/components/lib.typ"),
            include_str!("../../templates/components/lib.typ"),
        )
        .unwrap();
        let report = inspect_theme(&theme, Some("default"));
        assert!(report.ok(), "{:?}", report.errors);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn theme_check_reports_missing_declared_component() {
        let tmp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("typage-theme-missing-test-{}", std::process::id()));
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
}
