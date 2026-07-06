use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{load_config, Config, PluginConfig};
use crate::term;

const PLUGIN_FILE: &str = "typage.plugins.toml";

#[derive(Debug, Clone)]
pub struct PluginInstallOptions {
    pub root: PathBuf,
    pub name: String,
    pub package: Option<String>,
    pub version: Option<String>,
    pub git: Option<String>,
    pub path: Option<PathBuf>,
    pub command: Option<String>,
    pub force: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PluginFile {
    plugins: BTreeMap<String, PluginConfig>,
}

pub fn install(opts: PluginInstallOptions) -> Result<()> {
    validate_plugin_name(&opts.name)?;
    let root = crate::util::normalize_path(&opts.root)?;
    if !root.exists() {
        bail!("project root does not exist: {}", root.display());
    }
    if !root.is_dir() {
        bail!("project root is not a directory: {}", root.display());
    }
    if opts.path.is_some() && opts.git.is_some() {
        bail!("--path and --git cannot be used together");
    }
    let path = plugin_file_path(&root);
    let mut file = read_plugin_file(&path)?;
    if file.plugins.contains_key(&opts.name) && !opts.force {
        bail!(
            "plugin {:?} already exists in {}; pass --force to replace it",
            opts.name,
            path.display()
        );
    }

    let package = opts
        .package
        .clone()
        .unwrap_or_else(|| default_package_name(&opts.name));
    let source = if opts.command.is_some() {
        "command"
    } else if opts.path.is_some() {
        "path"
    } else if opts.git.is_some() {
        "git"
    } else {
        "crates.io"
    };

    if opts.command.is_none() {
        install_with_cargo(&package, &opts)?;
    }

    let mut plugin = PluginConfig {
        source: source.to_string(),
        package: Some(package.clone()),
        version: opts.version.clone(),
        git: opts.git.clone(),
        path: opts.path.clone(),
        command: opts.command.clone().or(Some(package)),
        ..PluginConfig::default()
    };
    if source == "command" {
        plugin.package = opts.package.clone();
    }
    file.plugins.insert(opts.name.clone(), plugin);
    write_plugin_file(&path, &file)?;
    println!("installed plugin {} in {}", opts.name, path.display());
    Ok(())
}

pub fn list(root: PathBuf) -> Result<()> {
    let root = crate::util::normalize_path(&root)?;
    let cfg = load_config(&root)?;
    if cfg.plugins.is_empty() {
        println!("no plugins configured");
        return Ok(());
    }
    println!("configured plugins:");
    for (name, plugin) in &cfg.plugins {
        let status = if plugin.disabled {
            "disabled"
        } else {
            "enabled"
        };
        println!("  {name:20} {status:8} {}", plugin_command(name, plugin));
    }
    Ok(())
}

pub fn doctor(root: PathBuf) -> Result<()> {
    let root = crate::util::normalize_path(&root)?;
    let cfg = load_config(&root)?;
    let errors = diagnose_plugins(&root, &cfg);
    if errors.is_empty() {
        println!("plugins: ok");
        return Ok(());
    }
    bail!(
        "plugin doctor found {} issue(s)\n\n{}",
        errors.len(),
        errors.join("\n")
    )
}

pub fn run(root: PathBuf, name: String, args: Vec<String>) -> Result<()> {
    let root = crate::util::normalize_path(&root)?;
    let cfg = load_config(&root)?;
    let Some(plugin) = cfg.plugins.get(&name) else {
        bail!("unknown plugin {name:?}");
    };
    if plugin.disabled {
        bail!("plugin {name:?} is disabled");
    }
    let args = if args.is_empty() {
        vec!["manifest".to_string(), "--json".to_string()]
    } else {
        args
    };
    let status = plugin_command_base(&root, &name, plugin)
        .args(args)
        .envs(plugin_env(&root, &name, plugin)?)
        .status()
        .with_context(|| format!("failed to start plugin {name}"))?;
    if !status.success() {
        bail!("plugin {name:?} failed with {status}");
    }
    Ok(())
}

pub fn run_build_plugins(root: &Path, cfg: &Config, quiet: bool) -> Result<()> {
    if std::env::var_os("TYPAGE_RUNNING_PLUGIN").is_some() {
        return Ok(());
    }
    for (name, plugin) in &cfg.plugins {
        if plugin.disabled {
            continue;
        }
        let validation_errors = validate_plugin_config(name, plugin);
        if !validation_errors.is_empty() {
            bail!(
                "invalid plugin configuration for {name}\n\n{}",
                validation_errors.join("\n")
            );
        }
        if !quiet {
            println!("{} {} {}", term::bold("typage"), term::cyan("plugin"), name);
        }
        let mut cmd = plugin_command_base(root, name, plugin);
        cmd.arg("generate")
            .arg("--root")
            .arg(root)
            .arg("--name")
            .arg(name)
            .args(&plugin.args)
            .envs(plugin_env(root, name, plugin)?)
            .env("TYPAGE_RUNNING_PLUGIN", "1");
        let status = cmd
            .status()
            .with_context(|| format!("failed to start plugin {name}"))?;
        if !status.success() {
            bail!("plugin {name:?} failed with {status}");
        }
    }
    Ok(())
}

pub fn diagnose_plugins(root: &Path, cfg: &Config) -> Vec<String> {
    let mut errors = Vec::new();
    for (name, plugin) in &cfg.plugins {
        if plugin.disabled {
            continue;
        }
        let validation_errors = validate_plugin_config(name, plugin);
        if !validation_errors.is_empty() {
            errors.extend(
                validation_errors
                    .into_iter()
                    .map(|err| format!("plugin {name}: {err}")),
            );
            continue;
        }
        match plugin_command_base(root, name, plugin)
            .arg("manifest")
            .arg("--json")
            .envs(match plugin_env(root, name, plugin) {
                Ok(env) => env,
                Err(err) => {
                    errors.push(format!(
                        "plugin {name}: failed to prepare environment: {err:?}"
                    ));
                    continue;
                }
            })
            .output()
        {
            Ok(output) if output.status.success() => {
                if let Err(err) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    errors.push(format!(
                        "plugin {name}: manifest did not return valid JSON: {err}"
                    ));
                }
            }
            Ok(output) => errors.push(format!(
                "plugin {name}: manifest failed with {}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )),
            Err(err) => errors.push(format!(
                "plugin {name}: failed to run {}: {err}",
                plugin_command(name, plugin)
            )),
        }
    }
    errors
}

pub fn watch_roots(root: &Path, cfg: &Config) -> Vec<PathBuf> {
    let mut seen = BTreeSet::<PathBuf>::new();
    let mut roots = Vec::new();
    for plugin in cfg.plugins.values() {
        if plugin.disabled {
            continue;
        }
        for path in &plugin.watch {
            if !is_safe_project_path(path) {
                continue;
            }
            let rel = watch_prefix(path);
            let full = root.join(rel);
            if seen.insert(full.clone()) {
                roots.push(full);
            }
        }
    }
    roots
}

fn install_with_cargo(package: &str, opts: &PluginInstallOptions) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("install");
    if opts.force {
        cmd.arg("--force");
    }
    match (&opts.path, &opts.git) {
        (Some(path), None) => {
            cmd.arg("--path").arg(path);
        }
        (None, Some(git)) => {
            cmd.arg("--git").arg(git).arg(package);
        }
        (None, None) => {
            cmd.arg(package);
            if let Some(version) = &opts.version {
                cmd.arg("--version").arg(version);
            }
        }
        (Some(_), Some(_)) => bail!("--path and --git cannot be used together"),
    }
    cmd.arg("--locked");
    println!("installing plugin with {:?}", cmd);
    let status = cmd.status().context("failed to start cargo install")?;
    if !status.success() {
        bail!("cargo install failed with {status}");
    }
    Ok(())
}

fn read_plugin_file(path: &Path) -> Result<PluginFile> {
    if !path.exists() {
        return Ok(PluginFile::default());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_plugin_file(path: &Path, file: &PluginFile) -> Result<()> {
    let raw = toml::to_string_pretty(file)?;
    fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn plugin_file_path(root: &Path) -> PathBuf {
    root.join(PLUGIN_FILE)
}

fn validate_plugin_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("plugin name must not be empty");
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        bail!("plugin name must contain only ASCII letters, digits, '_' or '-': {name}");
    }
    Ok(())
}

fn validate_plugin_config(name: &str, plugin: &PluginConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(err) = validate_plugin_name(name) {
        errors.push(err.to_string());
    }
    if plugin
        .command
        .as_deref()
        .is_some_and(|command| command.trim().is_empty())
    {
        errors.push("command must not be empty".to_string());
    }
    for (kind, paths) in [("watch", &plugin.watch), ("outputs", &plugin.outputs)] {
        for path in paths {
            if !is_safe_project_path(path) {
                errors.push(format!(
                    "{kind} path must be relative and stay inside the project root: {}",
                    path.display()
                ));
            }
        }
    }
    errors
}

fn is_safe_project_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return false;
    }
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn default_package_name(name: &str) -> String {
    format!("typage-plugin-{}", name.replace('_', "-"))
}

fn plugin_command(name: &str, plugin: &PluginConfig) -> String {
    plugin
        .command
        .clone()
        .or_else(|| plugin.package.clone())
        .unwrap_or_else(|| default_package_name(name))
}

fn plugin_command_base(root: &Path, name: &str, plugin: &PluginConfig) -> Command {
    let command = plugin_command(name, plugin);
    let program = command_path(root, &command);
    let mut cmd = Command::new(program);
    cmd.current_dir(root);
    cmd
}

fn command_path(root: &Path, command: &str) -> PathBuf {
    let path = PathBuf::from(command);
    if path.is_absolute() || command.contains('/') || command.contains('\\') {
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    } else {
        path
    }
}

fn plugin_env(root: &Path, name: &str, plugin: &PluginConfig) -> Result<BTreeMap<String, String>> {
    let payload = json!({
        "name": name,
        "source": plugin.source,
        "package": plugin.package,
        "version": plugin.version,
        "git": plugin.git,
        "path": plugin.path,
        "watch": plugin.watch,
        "outputs": plugin.outputs,
        "config": plugin.config,
    });
    let mut env = BTreeMap::new();
    env.insert(
        "TYPAGE_ROOT".to_string(),
        root.to_string_lossy().to_string(),
    );
    env.insert("TYPAGE_PLUGIN_NAME".to_string(), name.to_string());
    env.insert(
        "TYPAGE_PLUGIN_CONFIG_JSON".to_string(),
        serde_json::to_string(&payload)?,
    );
    Ok(env)
}

fn watch_prefix(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let text = value.to_string_lossy();
                if text.contains('*') || text.contains('?') || text.contains('[') {
                    break;
                }
                out.push(value);
            }
            Component::CurDir => {}
            _ => break,
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_package_name_uses_plugin_prefix() {
        assert_eq!(
            default_package_name("typst_docs"),
            "typage-plugin-typst-docs"
        );
    }

    #[test]
    fn watch_prefix_uses_static_part_before_glob() {
        assert_eq!(
            watch_prefix(Path::new("src/**/*.typ")),
            PathBuf::from("src")
        );
        assert_eq!(
            watch_prefix(Path::new("packages/*/src")),
            PathBuf::from("packages")
        );
        assert_eq!(watch_prefix(Path::new("src")), PathBuf::from("src"));
    }

    #[test]
    fn plugin_paths_must_stay_inside_project() {
        let plugin = PluginConfig {
            watch: vec![PathBuf::from("../src")],
            outputs: vec![PathBuf::from("/tmp/out")],
            ..PluginConfig::default()
        };
        let errors = validate_plugin_config("docs", &plugin);
        assert!(errors.iter().any(|error| error.contains("watch path")));
        assert!(errors.iter().any(|error| error.contains("outputs path")));
    }

    #[cfg(unix)]
    #[test]
    fn run_build_plugins_invokes_generate_protocol() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!("typage-plugin-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let script = tmp.join("plugin.sh");
        let marker = tmp.join("generated.txt");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = generate ]; then echo \"$TYPAGE_PLUGIN_NAME\" > {}; exit 0; fi\nif [ \"$1\" = manifest ]; then echo '{{\"name\":\"test\"}}'; exit 0; fi\nexit 1\n",
                marker.display()
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let mut cfg = Config::default();
        cfg.plugins.insert(
            "demo".to_string(),
            PluginConfig {
                source: "command".to_string(),
                command: Some(script.to_string_lossy().to_string()),
                ..PluginConfig::default()
            },
        );

        run_build_plugins(&tmp, &cfg, true).unwrap();
        assert_eq!(fs::read_to_string(marker).unwrap().trim(), "demo");
        let _ = fs::remove_dir_all(tmp);
    }
}
