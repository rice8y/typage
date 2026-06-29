use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;

use crate::config::load_config;
use crate::term;
use crate::util::{normalize_path, write_if_changed};

const TYPST_CLI_VERSION: &str = "0.15.0";
const TYPAGE_INSTALL_COMMAND: &str = "cargo install typage --locked";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DeployTarget {
    /// Generate a GitHub Actions workflow for GitHub Pages.
    #[value(name = "github-pages")]
    GithubPages,
    /// Generate a wrangler.toml scaffold for Cloudflare Pages.
    #[value(name = "cloudflare-pages")]
    CloudflarePages,
    /// Generate a netlify.toml scaffold for Netlify.
    #[value(name = "netlify")]
    Netlify,
    /// Generate a vercel.json scaffold for Vercel.
    #[value(name = "vercel")]
    Vercel,
}

impl DeployTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            DeployTarget::GithubPages => "github-pages",
            DeployTarget::CloudflarePages => "cloudflare-pages",
            DeployTarget::Netlify => "netlify",
            DeployTarget::Vercel => "vercel",
        }
    }
}

impl fmt::Display for DeployTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn init(root: PathBuf, target: DeployTarget, cname: Option<String>, force: bool) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    let mut written = Vec::<PathBuf>::new();

    match target {
        DeployTarget::GithubPages => {
            let workflow = root.join(".github/workflows/deploy.yml");
            write_scaffold(&workflow, &github_pages_workflow(&cfg.out_dir), force)?;
            written.push(workflow);

            let nojekyll = root.join(&cfg.static_dir).join(".nojekyll");
            write_scaffold(&nojekyll, "", force)?;
            written.push(nojekyll);

            if let Some(cname) = cname.as_deref() {
                validate_cname(cname)?;
                let cname_path = root.join(&cfg.static_dir).join("CNAME");
                write_scaffold(&cname_path, &format!("{}\n", cname.trim()), force)?;
                written.push(cname_path);
            }
        }
        DeployTarget::CloudflarePages => {
            if let Some(cname) = cname.as_deref() {
                validate_cname(cname)?;
                println!("{} custom domains for Cloudflare Pages are normally configured in the Cloudflare dashboard", term::yellow("note"));
            }
            let wrangler = root.join("wrangler.toml");
            write_scaffold(&wrangler, &cloudflare_wrangler(&cfg.out_dir), force)?;
            written.push(wrangler);
        }
        DeployTarget::Netlify => {
            if let Some(cname) = cname.as_deref() {
                validate_cname(cname)?;
                println!("{} custom domains for Netlify are normally configured in the Netlify dashboard", term::yellow("note"));
            }
            let netlify = root.join("netlify.toml");
            write_scaffold(&netlify, &netlify_toml(&cfg.out_dir), force)?;
            written.push(netlify);
        }
        DeployTarget::Vercel => {
            if let Some(cname) = cname.as_deref() {
                validate_cname(cname)?;
                println!(
                    "{} custom domains for Vercel are normally configured in the Vercel dashboard",
                    term::yellow("note")
                );
            }
            let vercel = root.join("vercel.json");
            write_scaffold(&vercel, &vercel_json(&cfg.out_dir), force)?;
            written.push(vercel);
        }
    }

    println!(
        "{} deploy scaffold for {}",
        term::green("✓"),
        term::bold(target.to_string())
    );
    for path in written {
        println!("  {} {}", term::dim("write"), path.display());
    }
    Ok(())
}

pub fn doctor(root: PathBuf, target: Option<DeployTarget>) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    let out_root = root.join(&cfg.out_dir);
    let mut issues = Vec::<String>::new();

    println!("{} {}", term::bold("typage"), term::cyan("deploy doctor"));
    println!("root: {}", root.display());
    println!("out: {}", out_root.display());
    if out_root.exists() {
        println!("out: ok");
    } else {
        println!("out: missing; run `typage build` before deploying");
    }
    if cfg.base_url.trim().is_empty() {
        println!("{} base_url is empty; set it before publishing if you need absolute canonical URLs, feeds, or deployment previews", term::yellow("warning"));
    } else {
        println!("base_url: {}", cfg.base_url);
    }

    let targets = match target {
        Some(target) => vec![target],
        None => detect_targets(&root),
    };
    if targets.is_empty() {
        issues.push("no deployment scaffold found; run `typage deploy init <target>`".to_string());
    }

    for target in targets {
        match target {
            DeployTarget::GithubPages => check_file(
                &root.join(".github/workflows/deploy.yml"),
                "github-pages workflow",
                &mut issues,
            ),
            DeployTarget::CloudflarePages => check_file(
                &root.join("wrangler.toml"),
                "cloudflare-pages wrangler.toml",
                &mut issues,
            ),
            DeployTarget::Netlify => {
                check_file(&root.join("netlify.toml"), "netlify.toml", &mut issues)
            }
            DeployTarget::Vercel => {
                check_file(&root.join("vercel.json"), "vercel.json", &mut issues)
            }
        }
    }

    if issues.is_empty() {
        println!("{} deploy configuration looks ok", term::green("✓"));
        Ok(())
    } else {
        bail!(
            "deploy doctor found {} issue(s)\n\n{}",
            issues.len(),
            issues.join("\n")
        );
    }
}

fn detect_targets(root: &Path) -> Vec<DeployTarget> {
    let mut targets = Vec::new();
    if root.join(".github/workflows/deploy.yml").exists() {
        targets.push(DeployTarget::GithubPages);
    }
    if root.join("wrangler.toml").exists() {
        targets.push(DeployTarget::CloudflarePages);
    }
    if root.join("netlify.toml").exists() {
        targets.push(DeployTarget::Netlify);
    }
    if root.join("vercel.json").exists() {
        targets.push(DeployTarget::Vercel);
    }
    targets
}

fn check_file(path: &Path, label: &str, issues: &mut Vec<String>) {
    if path.exists() {
        println!("{label}: ok ({})", path.display());
    } else {
        issues.push(format!("missing {label}: {}", path.display()));
    }
}

fn write_scaffold(path: &Path, contents: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "refusing to overwrite {}. Re-run with --force",
            path.display()
        );
    }
    write_if_changed(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

fn validate_cname(cname: &str) -> Result<()> {
    let cname = cname.trim();
    if cname.is_empty() {
        bail!("CNAME must not be empty");
    }
    if cname.contains('/')
        || cname.contains('\\')
        || cname.contains(':')
        || cname.chars().any(char::is_whitespace)
    {
        bail!("invalid CNAME: {cname}");
    }
    Ok(())
}

fn github_pages_workflow(out_dir: &Path) -> String {
    let out = path_for_yaml(out_dir);
    let typage_install = TYPAGE_INSTALL_COMMAND;
    format!(
        r#"name: Deploy typage site to GitHub Pages

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: typst-community/setup-typst@v4
        with:
          typst-version: {TYPST_CLI_VERSION}
      - name: Install typage
        run: {typage_install}
      - name: Build site
        run: typage build --force --jobs 0
      - uses: actions/upload-pages-artifact@v3
        with:
          path: {out}

  deploy:
    environment:
      name: github-pages
      url: ${{{{ steps.deployment.outputs.page_url }}}}
    runs-on: ubuntu-latest
    needs: build
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
"#
    )
}

fn cloudflare_wrangler(out_dir: &Path) -> String {
    let out = path_for_toml(out_dir);
    let typst_install = typst_install_command();
    let typage_install = TYPAGE_INSTALL_COMMAND;
    format!(
        r#"# Cloudflare Pages scaffold for typage.
# Install command: {typst_install} && {typage_install}
# Build command: typage build --force --jobs 0
# Output directory: {out}
#
# For direct deploys with Wrangler:
#   npx wrangler pages deploy {out} --project-name <your-project>

name = "typage-site"
pages_build_output_dir = "{out}"
compatibility_date = "2026-06-24"
"#
    )
}

fn netlify_toml(out_dir: &Path) -> String {
    let out = path_for_toml(out_dir);
    let install = install_command();
    format!(
        r#"# Netlify scaffold for typage.
# Installs Typst CLI {TYPST_CLI_VERSION} and typage before building.
# Replace this command if your build image provides them another way.

[build]
command = "{install} && typage build --force --jobs 0"
publish = "{out}"
"#
    )
}

fn vercel_json(out_dir: &Path) -> String {
    let out = path_for_json(out_dir);
    let install = json_escape(&install_command());
    format!(
        r#"{{
  "$schema": "https://openapi.vercel.sh/vercel.json",
  "installCommand": "{install}",
  "buildCommand": "typage build --force --jobs 0",
  "outputDirectory": "{out}",
  "cleanUrls": true
}}
"#
    )
}

fn typst_install_command() -> String {
    format!("cargo install typst-cli --locked --version {TYPST_CLI_VERSION}")
}

fn install_command() -> String {
    format!("{} && {TYPAGE_INSTALL_COMMAND}", typst_install_command())
}

fn path_for_yaml(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_for_toml(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"")
}

fn path_for_json(path: &Path) -> String {
    json_escape(&path.to_string_lossy().replace('\\', "/"))
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("typage-deploy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("static")).unwrap();
        fs::write(
            root.join("config.toml"),
            "title = \"Deploy Test\"\nbase_url = \"https://example.com\"\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn github_pages_scaffold_writes_workflow_and_static_files() {
        let root = temp_project("github");
        init(
            root.clone(),
            DeployTarget::GithubPages,
            Some("example.com".to_string()),
            false,
        )
        .unwrap();
        assert!(root.join(".github/workflows/deploy.yml").exists());
        let workflow = fs::read_to_string(root.join(".github/workflows/deploy.yml")).unwrap();
        assert!(workflow.contains("uses: typst-community/setup-typst@v4"));
        assert!(workflow.contains("typst-version: 0.15.0"));
        assert!(workflow.contains("run: cargo install typage --locked"));
        assert!(root.join("static/.nojekyll").exists());
        assert_eq!(
            fs::read_to_string(root.join("static/CNAME")).unwrap(),
            "example.com\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn vercel_scaffold_writes_vercel_json() {
        let root = temp_project("vercel");
        init(root.clone(), DeployTarget::Vercel, None, false).unwrap();
        let path = root.join("vercel.json");
        assert!(path.exists());
        let raw = fs::read_to_string(path).unwrap();
        assert!(raw.contains("cargo install typst-cli --locked --version 0.15.0"));
        assert!(raw.contains("cargo install typage --locked"));
        assert!(raw.contains("\"buildCommand\": \"typage build --force --jobs 0\""));
        assert!(raw.contains("\"outputDirectory\": \"public\""));
        assert!(raw.contains("\"cleanUrls\": true"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn netlify_scaffold_installs_typst_and_typage() {
        let root = temp_project("netlify");
        init(root.clone(), DeployTarget::Netlify, None, false).unwrap();
        let raw = fs::read_to_string(root.join("netlify.toml")).unwrap();
        assert!(raw.contains("cargo install typst-cli --locked --version 0.15.0"));
        assert!(raw.contains("cargo install typage --locked"));
        assert!(raw.contains("typage build --force --jobs 0"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_to_overwrite_deploy_scaffold_without_force() {
        let root = temp_project("overwrite");
        init(root.clone(), DeployTarget::Netlify, None, false).unwrap();
        let err = init(root.clone(), DeployTarget::Netlify, None, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to overwrite"));
        init(root.clone(), DeployTarget::Netlify, None, true).unwrap();
        let _ = fs::remove_dir_all(root);
    }
}
