mod build;
mod config;
mod deploy;
mod model;
mod serve;
mod term;
mod util;
mod watch;

use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

use crate::config::load_config;

#[derive(Parser, Debug)]
#[command(name = "typage")]
#[command(version, about = "A Rust SSG powered by Typst HTML Export")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a sample project.
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Create a new content page.
    New {
        /// Page path relative to content_dir. `.typ` is appended when omitted.
        path: PathBuf,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        draft: bool,
    },
    /// Check configuration, routes, links, and Typst availability without building.
    Doctor {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        typst: Option<String>,
        #[arg(long)]
        drafts: bool,
    },
    /// Run a script from [scripts], similar to `npm run`.
    Run {
        /// Script name. Without a name, prints available scripts.
        script: Option<String>,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Extra arguments appended after the script.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Alias for `run dev`.
    Dev {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Alias for `run preview`.
    Preview {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Alias for `doctor`.
    Check {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Build the site.
    Build {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        drafts: bool,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        typst: Option<String>,
        #[arg(long)]
        pdf: bool,
        /// Keep building remaining pages after a Typst compilation failure.
        #[arg(long)]
        keep_going: bool,
        /// Number of parallel Typst compile jobs. 0 means auto.
        #[arg(long)]
        jobs: Option<usize>,
        /// Print timing breakdown and slowest compile jobs.
        #[arg(long)]
        profile: bool,
        /// Explain why pages are rebuilt or skipped.
        #[arg(long)]
        explain: bool,
        /// Reduce build output.
        #[arg(long, short)]
        quiet: bool,
        /// Print more detailed build output.
        #[arg(long, short)]
        verbose: bool,
        /// Disable configured hooks for this invocation.
        #[arg(long)]
        no_hooks: bool,
    },
    /// Watch content/templates/static and rebuild on changes.
    Watch {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        drafts: bool,
        #[arg(long)]
        pdf: bool,
        /// Number of parallel Typst compile jobs. 0 means auto.
        #[arg(long)]
        jobs: Option<usize>,
        /// Disable configured hooks for this invocation.
        #[arg(long)]
        no_hooks: bool,
    },
    /// Build and serve public/ through a small static HTTP server.
    Serve {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "127.0.0.1:1111")]
        addr: String,
        #[arg(long)]
        drafts: bool,
        #[arg(long)]
        pdf: bool,
        /// Rebuild on changes and inject a tiny reload script into served HTML.
        #[arg(long)]
        live_reload: bool,
        /// Number of parallel Typst compile jobs. 0 means auto.
        #[arg(long)]
        jobs: Option<usize>,
        /// Disable configured hooks for this invocation.
        #[arg(long)]
        no_hooks: bool,
    },
    /// Generate a Typst bundle source and compile it with --format bundle.
    Bundle {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        drafts: bool,
        #[arg(long)]
        typst: Option<String>,
    },
    /// Generate deployment configuration scaffolds.
    Deploy {
        #[command(subcommand)]
        command: DeployCommand,
    },
    /// Theme management helpers.
    Theme {
        #[command(subcommand)]
        command: ThemeCommand,
    },
    /// Remove public/ and .typage/.
    Clean {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum DeployCommand {
    /// Generate deployment configuration for a target.
    Init {
        target: deploy::DeployTarget,
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Custom domain for GitHub Pages. Creates static/CNAME.
        #[arg(long)]
        cname: Option<String>,
        /// Overwrite existing scaffold files.
        #[arg(long)]
        force: bool,
        /// Disable configured hooks for this invocation.
        #[arg(long)]
        no_hooks: bool,
    },
    /// Check deployment scaffold files.
    Doctor {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        target: Option<deploy::DeployTarget>,
        /// Disable configured hooks for this invocation.
        #[arg(long)]
        no_hooks: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ThemeCommand {
    /// Create a reusable theme skeleton under themes/<name>.
    New {
        name: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// List installed themes under themes/.
    List {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Show theme metadata, components, and extra keys.
        #[arg(long, short)]
        verbose: bool,
    },
    /// Show metadata for a theme, or the active theme when omitted.
    Info {
        name: Option<String>,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Validate theme metadata, templates, components, and compatibility.
    Check {
        name: Option<String>,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path } => build::init_project(&path),
        Command::New {
            root,
            path,
            title,
            date,
            draft,
        } => build::new_page(root, path, title, date, draft),
        Command::Doctor {
            root,
            typst,
            drafts,
        } => build::doctor(root, typst, drafts),
        Command::Run { script, root, args } => run_script(root, script, args),
        Command::Dev { root } => run_script(root, Some("dev".to_string()), Vec::new()),
        Command::Preview { root } => run_script(root, Some("preview".to_string()), Vec::new()),
        Command::Check { root } => build::doctor(root, None, false),
        Command::Build {
            root,
            drafts,
            force,
            typst,
            pdf,
            keep_going,
            jobs,
            profile,
            explain,
            quiet,
            verbose,
            no_hooks,
        } => {
            run_hook(&root, "pre_build", no_hooks)?;
            let opts = build::BuildOptions {
                root: root.clone(),
                drafts,
                force,
                typst_override: typst,
                pdf,
                keep_going,
                jobs,
                profile,
                explain,
                quiet,
                verbose,
            };
            build::build_site(&opts).map(|_| ())?;
            run_hook(&root, "post_build", no_hooks)
        }
        Command::Watch {
            root,
            drafts,
            pdf,
            jobs,
            no_hooks: _,
        } => watch::watch(root, drafts, pdf, jobs),
        Command::Serve {
            root,
            addr,
            drafts,
            pdf,
            live_reload,
            jobs,
            no_hooks,
        } => {
            run_hook(&root, "pre_serve", no_hooks)?;
            let opts = build::BuildOptions {
                root: root.clone(),
                drafts,
                force: false,
                typst_override: None,
                pdf,
                keep_going: live_reload,
                jobs,
                profile: false,
                explain: false,
                quiet: false,
                verbose: false,
            };
            build::build_site(&opts)?;
            let result = serve::serve(root.clone(), addr, live_reload, drafts, pdf, jobs);
            if result.is_ok() {
                run_hook(&root, "post_serve", no_hooks)?;
            }
            result
        }
        Command::Bundle {
            root,
            drafts,
            typst,
        } => build::bundle_site(root, drafts, typst),
        Command::Deploy { command } => match command {
            DeployCommand::Init {
                root,
                target,
                cname,
                force,
                no_hooks,
            } => {
                run_hook(&root, "pre_deploy", no_hooks)?;
                deploy::init(root.clone(), target, cname, force)?;
                run_hook(&root, "post_deploy", no_hooks)
            }
            DeployCommand::Doctor {
                root,
                target,
                no_hooks,
            } => {
                run_hook(&root, "pre_deploy", no_hooks)?;
                deploy::doctor(root.clone(), target)?;
                run_hook(&root, "post_deploy", no_hooks)
            }
        },
        Command::Theme { command } => match command {
            ThemeCommand::New { name, root } => build::new_theme(root, name),
            ThemeCommand::List { root, verbose } => build::list_themes(root, verbose),
            ThemeCommand::Info { root, name } => build::theme_info(root, name),
            ThemeCommand::Check { root, name } => build::theme_check(root, name),
        },
        Command::Clean { root } => build::clean(root),
    }
}

fn run_hook(root: &PathBuf, name: &str, no_hooks: bool) -> Result<()> {
    if no_hooks || std::env::var_os("TYPAGE_RUNNING_HOOK").is_some() {
        return Ok(());
    }
    let cfg = load_config(root)?;
    let Some(command) = cfg.hooks.get(name) else {
        return Ok(());
    };
    let mut parts = split_script(command)?;
    let program = parts.remove(0);
    println!(
        "{} {} {}",
        crate::term::bold("typage"),
        crate::term::cyan("hook"),
        name
    );
    println!("  {} {}", crate::term::dim("run"), command);
    let mut cmd = if program == "typage" {
        ProcessCommand::new(std::env::current_exe()?)
    } else {
        ProcessCommand::new(&program)
    };
    cmd.current_dir(root)
        .args(parts)
        .env("TYPAGE_RUNNING_HOOK", "1");
    let status = cmd.status()?;
    if !status.success() {
        bail!("hook {name:?} failed with {status}");
    }
    Ok(())
}

fn run_script(root: PathBuf, script: Option<String>, extra_args: Vec<String>) -> Result<()> {
    let cfg = load_config(&root)?;
    let scripts = merged_scripts(&cfg.scripts);
    let Some(script) = script else {
        println!("available scripts:");
        for (name, command) in scripts {
            println!("  {name:10} {command}");
        }
        return Ok(());
    };
    let Some(command) = scripts.get(&script) else {
        bail!("unknown script {script:?}. Run `typage run` to list scripts");
    };
    let mut args = split_script(command)?;
    if !args.iter().any(|arg| arg == "--root") {
        args.push("--root".to_string());
        args.push(root.to_string_lossy().to_string());
    }
    args.extend(extra_args);
    let exe = std::env::current_exe()?;
    println!("typage run {script}: typage {}", args.join(" "));
    let status = ProcessCommand::new(exe).args(args).status()?;
    if !status.success() {
        bail!("script {script:?} failed with {status}");
    }
    Ok(())
}

fn merged_scripts(
    user: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    let mut scripts = std::collections::BTreeMap::new();
    scripts.insert("build".to_string(), "build".to_string());
    scripts.insert("dev".to_string(), "serve --live-reload".to_string());
    scripts.insert("preview".to_string(), "serve".to_string());
    scripts.insert("check".to_string(), "doctor".to_string());
    scripts.insert("clean".to_string(), "clean".to_string());
    for (k, v) in user {
        scripts.insert(k.clone(), v.clone());
    }
    scripts
}

fn split_script(command: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;
    for ch in command.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(cur.clone());
                cur.clear();
            }
        } else {
            cur.push(ch);
        }
    }
    if quote.is_some() {
        bail!("unterminated quote in script: {command}");
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        bail!("empty script command");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_script_like_shell_words() {
        let got = split_script("serve --addr \"127.0.0.1:1111\" --live-reload").unwrap();
        assert_eq!(
            got,
            vec!["serve", "--addr", "127.0.0.1:1111", "--live-reload"]
        );
    }
}
