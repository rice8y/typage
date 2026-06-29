use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{RecursiveMode, Watcher};

use crate::build::{self, BuildOptions};
use crate::config::load_config;
use crate::util::normalize_path;

pub fn watch(root: PathBuf, drafts: bool, pdf: bool, jobs: Option<usize>) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    build::build_site(&BuildOptions {
        root: root.clone(),
        drafts,
        force: false,
        typst_override: None,
        pdf,
        keep_going: false,
        jobs,
        profile: false,
        explain: false,
        quiet: false,
        verbose: false,
    })?;

    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;

    for path in [&cfg.content_dir, &cfg.templates_dir, &cfg.static_dir] {
        let full = root.join(path);
        if full.exists() {
            watcher.watch(&full, RecursiveMode::Recursive)?;
            println!("watching {}", full.display());
        }
    }
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        let full = root.join("themes").join(theme);
        if full.exists() {
            watcher.watch(&full, RecursiveMode::Recursive)?;
            println!("watching {}", full.display());
        }
    }

    let mut last = Instant::now() - Duration::from_secs(10);
    loop {
        match rx.recv() {
            Ok(Ok(_event)) => {
                if last.elapsed() < Duration::from_millis(250) {
                    continue;
                }
                last = Instant::now();
                println!("change detected; rebuilding");
                if let Err(err) = build::build_site(&BuildOptions {
                    root: root.clone(),
                    drafts,
                    force: false,
                    typst_override: None,
                    pdf,
                    keep_going: false,
                    jobs,
                    profile: false,
                    explain: false,
                    quiet: false,
                    verbose: false,
                }) {
                    eprintln!("build failed: {err:?}");
                }
            }
            Ok(Err(err)) => eprintln!("watch error: {err}"),
            Err(err) => return Err(err.into()),
        }
    }
}
