use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::config::PluginConfig;
use crate::plugin::{self, PluginRegisterOptions};
use crate::util::{normalize_path, remove_dir_if_exists};

const STARTER_FILE: &str = "typage.starter.toml";
const TYPAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct StarterInitOptions {
    pub root: PathBuf,
    pub source: String,
    pub install_plugins: bool,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct StarterAddOptions {
    pub root: PathBuf,
    pub source: String,
    pub install_plugins: bool,
    pub force: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StarterManifest {
    name: Option<String>,
    description: Option<String>,
    min_typage: Option<String>,
    files: StarterFiles,
    #[serde(alias = "plugin")]
    plugins: BTreeMap<String, PluginConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StarterFiles {
    root: Option<PathBuf>,
    exclude: Vec<PathBuf>,
}

struct PreparedStarter {
    root: PathBuf,
    cleanup: Option<PathBuf>,
}

enum StarterSource {
    Local(PathBuf),
    Git { url: String, rev: Option<String> },
}

pub fn init(opts: StarterInitOptions) -> Result<()> {
    let root = normalize_path(&opts.root)?;
    fs::create_dir_all(&root)
        .with_context(|| format!("failed to create project root {}", root.display()))?;
    apply_starter(&root, &opts.source, opts.install_plugins, opts.force)?;
    println!("initialized {}", root.display());
    Ok(())
}

pub fn add(opts: StarterAddOptions) -> Result<()> {
    let root = normalize_path(&opts.root)?;
    if !root.exists() {
        bail!("project root does not exist: {}", root.display());
    }
    if !root.is_dir() {
        bail!("project root is not a directory: {}", root.display());
    }
    apply_starter(&root, &opts.source, opts.install_plugins, opts.force)
}

fn apply_starter(root: &Path, source: &str, install_plugins: bool, force: bool) -> Result<()> {
    let prepared = prepare_source(source)?;
    let manifest = read_manifest(&prepared.root)?;
    ensure_min_typage(&manifest)?;
    let files_root = starter_files_root(&prepared.root, &manifest)?;
    let written = copy_starter_files(&files_root, root, &manifest, force)?;

    for (name, plugin) in manifest.plugins {
        plugin::register(PluginRegisterOptions {
            root: root.to_path_buf(),
            name,
            plugin,
            install: install_plugins,
            force,
        })?;
    }

    if let Some(path) = prepared.cleanup {
        remove_dir_if_exists(&path)?;
    }

    let label = manifest.name.as_deref().unwrap_or(source);
    if let Some(description) = manifest.description.as_deref() {
        println!("applied starter {label}: {description}");
    } else {
        println!("applied starter {label}");
    }
    println!("write {} file(s)", written);
    Ok(())
}

fn prepare_source(source: &str) -> Result<PreparedStarter> {
    match parse_source(source)? {
        StarterSource::Local(path) => {
            let root = normalize_path(&path)?;
            if !root.exists() {
                bail!("starter path does not exist: {}", root.display());
            }
            if !root.is_dir() {
                bail!("starter path is not a directory: {}", root.display());
            }
            Ok(PreparedStarter {
                root,
                cleanup: None,
            })
        }
        StarterSource::Git { url, rev } => {
            let root = temp_starter_dir()?;
            clone_starter(&url, rev.as_deref(), &root)?;
            Ok(PreparedStarter {
                root: root.clone(),
                cleanup: Some(root),
            })
        }
    }
}

fn parse_source(source: &str) -> Result<StarterSource> {
    let source = source.trim();
    if source.is_empty() {
        bail!("starter source must not be empty");
    }
    let (base, rev) = split_ref(source);
    if let Some(repo) = base.strip_prefix("github:") {
        validate_github_repo(repo)?;
        return Ok(StarterSource::Git {
            url: format!("https://github.com/{repo}.git"),
            rev,
        });
    }
    if let Some(url) = base.strip_prefix("git+") {
        return Ok(StarterSource::Git {
            url: url.to_string(),
            rev,
        });
    }
    if looks_like_git_source(base) {
        return Ok(StarterSource::Git {
            url: base.to_string(),
            rev,
        });
    }
    Ok(StarterSource::Local(PathBuf::from(base)))
}

fn split_ref(source: &str) -> (&str, Option<String>) {
    match source.split_once('#') {
        Some((base, rev)) if !rev.trim().is_empty() => (base, Some(rev.trim().to_string())),
        Some((base, _)) => (base, None),
        None => (source, None),
    }
}

fn looks_like_git_source(source: &str) -> bool {
    source.ends_with(".git")
        || source.starts_with("https://")
        || source.starts_with("http://")
        || source.starts_with("ssh://")
        || source.starts_with("git@")
}

fn validate_github_repo(repo: &str) -> Result<()> {
    let parts = repo.split('/').collect::<Vec<_>>();
    if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
        bail!("GitHub starter sources must look like github:owner/repo");
    }
    if !repo
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
    {
        bail!("GitHub starter source contains unsupported characters: {repo}");
    }
    Ok(())
}

fn temp_starter_dir() -> Result<PathBuf> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::env::temp_dir().join(format!("typage-starter-{}-{millis}", std::process::id()));
    remove_dir_if_exists(&path)?;
    Ok(path)
}

fn clone_starter(url: &str, rev: Option<&str>, dst: &Path) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("clone").arg("--depth").arg("1");
    if let Some(rev) = rev {
        cmd.arg("--branch").arg(rev);
    }
    cmd.arg(url).arg(dst);
    println!("fetching starter with {:?}", cmd);
    let status = cmd.status().context("failed to start git clone")?;
    if !status.success() {
        bail!("git clone failed with {status}");
    }
    Ok(())
}

fn read_manifest(root: &Path) -> Result<StarterManifest> {
    let path = root.join(STARTER_FILE);
    if !path.exists() {
        return Ok(StarterManifest::default());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn ensure_min_typage(manifest: &StarterManifest) -> Result<()> {
    let Some(min) = manifest.min_typage.as_deref().filter(|min| !min.is_empty()) else {
        return Ok(());
    };
    let Some(required) = parse_version(min) else {
        bail!("invalid min_typage in starter manifest: {min}");
    };
    let Some(current) = parse_version(TYPAGE_VERSION) else {
        bail!("failed to parse typage version: {TYPAGE_VERSION}");
    };
    if current < required {
        bail!("starter requires Typage {min} or later; current version is {TYPAGE_VERSION}");
    }
    Ok(())
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim().strip_prefix('v').unwrap_or(value.trim());
    let core = value.split_once('-').map_or(value, |(core, _)| core);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn starter_files_root(source_root: &Path, manifest: &StarterManifest) -> Result<PathBuf> {
    if let Some(root) = &manifest.files.root {
        validate_project_path("files.root", root)?;
        let full = source_root.join(root);
        if !full.is_dir() {
            bail!("starter files root does not exist: {}", full.display());
        }
        return Ok(full);
    }
    let files = source_root.join("files");
    if files.is_dir() {
        Ok(files)
    } else {
        Ok(source_root.to_path_buf())
    }
}

fn copy_starter_files(
    source: &Path,
    target: &Path,
    manifest: &StarterManifest,
    force: bool,
) -> Result<usize> {
    let excludes = manifest
        .files
        .exclude
        .iter()
        .map(|path| normalized_project_path(path))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut written = 0usize;
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry?;
        let from = entry.path();
        let rel = from.strip_prefix(source)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        if is_default_excluded(rel) || is_excluded(rel, &excludes) {
            continue;
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = normalized_project_path(rel)?;
        let to = target.join(&rel);
        if copy_starter_file(from, &to, force)? {
            written += 1;
        }
    }
    Ok(written)
}

fn is_default_excluded(path: &Path) -> bool {
    path == Path::new(STARTER_FILE)
        || path == Path::new("typage.plugins.toml")
        || path.components().next().is_some_and(
            |component| matches!(component, Component::Normal(value) if value == ".git"),
        )
}

fn is_excluded(path: &Path, excludes: &BTreeSet<PathBuf>) -> bool {
    excludes
        .iter()
        .any(|exclude| path == exclude || path.starts_with(exclude))
}

fn copy_starter_file(from: &Path, to: &Path, force: bool) -> Result<bool> {
    let bytes = fs::read(from).with_context(|| format!("failed to read {}", from.display()))?;
    match fs::symlink_metadata(to) {
        Ok(meta) => {
            let kind = meta.file_type();
            if kind.is_file() {
                if fs::read(to).unwrap_or_default() == bytes {
                    return Ok(false);
                }
                if !force {
                    bail!("refusing to overwrite existing file: {}", to.display());
                }
            } else if kind.is_symlink() {
                if !force {
                    bail!("refusing to overwrite existing symlink: {}", to.display());
                }
                fs::remove_file(to)
                    .with_context(|| format!("failed to remove symlink {}", to.display()))?;
            } else {
                bail!("refusing to overwrite non-file path: {}", to.display());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", to.display())),
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(to, bytes).with_context(|| format!("failed to write {}", to.display()))?;
    println!("write {}", to.display());
    Ok(true)
}

fn normalized_project_path(path: &Path) -> Result<PathBuf> {
    validate_project_path("starter path", path)?;
    let mut out = PathBuf::new();
    for component in path.components() {
        if let Component::Normal(value) = component {
            out.push(value);
        }
    }
    Ok(out)
}

fn validate_project_path(label: &str, path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!(
            "{label} must be a project-relative path: {}",
            path.display()
        );
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => bail!(
                "{label} must not contain parent, prefix, or root components: {}",
                path.display()
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_config;

    #[test]
    fn github_source_expands_to_git_url() {
        match parse_source("github:rice8y/typage-starter-typst-docs#v1").unwrap() {
            StarterSource::Git { url, rev } => {
                assert_eq!(
                    url,
                    "https://github.com/rice8y/typage-starter-typst-docs.git"
                );
                assert_eq!(rev.as_deref(), Some("v1"));
            }
            StarterSource::Local(_) => panic!("expected git source"),
        }
    }

    #[test]
    fn add_copies_files_and_registers_plugins() {
        let tmp = std::env::temp_dir().join(format!("typage-starter-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let starter = tmp.join("starter");
        let project = tmp.join("project");
        fs::create_dir_all(starter.join("files/content")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(
            starter.join(STARTER_FILE),
            r#"
name = "demo"
min_typage = "0.1.4"

[plugins.typst_docs]
package = "typage-plugin-typst-docs"
version = "0.1.0"
watch = ["package/**/*.typ"]
outputs = ["content/api"]

[plugins.typst_docs.config]
source_dir = "package"
out_dir = "content/api"
"#,
        )
        .unwrap();
        fs::write(starter.join("files/config.toml"), "title = \"Demo\"\n").unwrap();
        fs::write(starter.join("files/content/index.typ"), "= Demo\n").unwrap();

        add(StarterAddOptions {
            root: project.clone(),
            source: starter.to_string_lossy().to_string(),
            install_plugins: false,
            force: false,
        })
        .unwrap();

        assert_eq!(
            fs::read_to_string(project.join("content/index.typ")).unwrap(),
            "= Demo\n"
        );
        let cfg = load_config(&project).unwrap();
        let plugin = cfg.plugins.get("typst_docs").unwrap();
        assert_eq!(plugin.package.as_deref(), Some("typage-plugin-typst-docs"));
        assert_eq!(plugin.watch, vec![PathBuf::from("package/**/*.typ")]);
        assert_eq!(
            plugin
                .config
                .get("source_dir")
                .and_then(toml::Value::as_str),
            Some("package")
        );
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn add_refuses_to_overwrite_without_force() {
        let tmp = std::env::temp_dir().join(format!(
            "typage-starter-overwrite-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let starter = tmp.join("starter");
        let project = tmp.join("project");
        fs::create_dir_all(starter.join("files")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(starter.join("files/config.toml"), "title = \"Starter\"\n").unwrap();
        fs::write(project.join("config.toml"), "title = \"Existing\"\n").unwrap();

        let err = add(StarterAddOptions {
            root: project,
            source: starter.to_string_lossy().to_string(),
            install_plugins: false,
            force: false,
        })
        .unwrap_err()
        .to_string();

        assert!(err.contains("refusing to overwrite existing file"));
        let _ = fs::remove_dir_all(tmp);
    }
}
