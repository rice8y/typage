use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::build::{BuildOptions, BuildSession};
use crate::config::{load_config, Config};
use crate::model::BuildCache;
use crate::util::normalize_path;

const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct WatchSession {
    opts: BuildOptions,
    cfg: Config,
    no_hooks: bool,
    build: BuildSession,
    watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    dependencies: BTreeSet<PathBuf>,
    external_roots: BTreeSet<PathBuf>,
}

impl WatchSession {
    pub fn new(mut opts: BuildOptions, no_hooks: bool) -> Result<Self> {
        opts.root = normalize_path(&opts.root)?;
        let cfg = load_config(&opts.root)?;
        let (tx, events) = channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })?;
        // Root watching catches missing input directories and atomic-save renames.
        watcher.watch(&opts.root, RecursiveMode::Recursive)?;
        Ok(Self {
            opts,
            cfg,
            no_hooks,
            build: BuildSession::default(),
            watcher,
            events,
            dependencies: BTreeSet::new(),
            external_roots: BTreeSet::new(),
        })
    }

    pub fn rebuild(&mut self) -> Result<PathBuf> {
        if let Ok(cfg) = load_config(&self.opts.root) {
            self.cfg = cfg;
        }
        crate::run_hook(&self.opts.root, "pre_build", self.no_hooks)?;
        let result = self.build.build(&self.opts);
        self.refresh_dependencies()?;
        result?;
        crate::run_hook(&self.opts.root, "post_build", self.no_hooks)?;
        Ok(self.opts.root.join(&self.cfg.out_dir))
    }

    fn refresh_dependencies(&mut self) -> Result<()> {
        let cache_path = self
            .opts
            .root
            .join(&self.cfg.cache_dir)
            .join("build-cache.json");
        if let Ok(raw) = std::fs::read(cache_path) {
            if let Ok(cache) = serde_json::from_slice::<BuildCache>(&raw) {
                self.dependencies = cache
                    .entries
                    .values()
                    .filter_map(|entry| entry.dependencies.as_ref())
                    .flat_map(|deps| deps.keys().cloned())
                    .collect();
            }
        }
        let external = self
            .dependencies
            .iter()
            .filter(|path| !path.starts_with(&self.opts.root))
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<BTreeSet<_>>();
        for path in external.difference(&self.external_roots) {
            if path.exists() {
                self.watcher.watch(path, RecursiveMode::NonRecursive)?;
            }
        }
        for path in self.external_roots.difference(&external) {
            let _ = self.watcher.unwatch(path);
        }
        self.external_roots = external;
        Ok(())
    }

    fn relevant(&self, event: &Event) -> bool {
        !event.kind.is_access()
            && event
                .paths
                .iter()
                .any(|path| relevant_path(&self.opts.root, &self.cfg, &self.dependencies, path))
    }

    pub fn run(mut self, mut rebuilt: impl FnMut(PathBuf)) -> Result<()> {
        let mut pending = Pending::default();
        loop {
            let event = match pending.remaining(Instant::now()) {
                Some(wait) => self.events.recv_timeout(wait),
                None => self
                    .events
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match event {
                Ok(Ok(event)) if self.relevant(&event) => pending.changed(Instant::now()),
                Ok(Err(error)) => eprintln!("watch error: {error}"),
                Ok(_) => (),
                Err(RecvTimeoutError::Timeout) => {
                    pending = Pending::default();
                    println!("change detected; rebuilding");
                    match self.rebuild() {
                        Ok(public) => rebuilt(public),
                        Err(error) => eprintln!("build failed: {error:?}"),
                    }
                    // Events arriving during compilation remain in the channel.
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[derive(Default)]
struct Pending {
    last_change: Option<Instant>,
}

impl Pending {
    fn changed(&mut self, now: Instant) {
        self.last_change = Some(now);
    }
    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.last_change
            .map(|last| DEBOUNCE.saturating_sub(now.saturating_duration_since(last)))
    }
}

fn relevant_path(root: &Path, cfg: &Config, dependencies: &BTreeSet<PathBuf>, path: &Path) -> bool {
    if path.starts_with(root.join(&cfg.out_dir))
        || path.starts_with(root.join(&cfg.cache_dir))
        || path.starts_with(root.join(".git"))
    {
        return false;
    }
    if ["config.toml", "typage.plugins.toml"]
        .iter()
        .any(|name| path == root.join(name))
    {
        return true;
    }
    let mut inputs = vec![
        root.join(&cfg.content_dir),
        root.join(&cfg.templates_dir),
        root.join(&cfg.static_dir),
    ];
    if let Some(theme) = &cfg.theme {
        inputs.push(root.join("themes").join(theme));
    }
    inputs.extend(crate::plugin::watch_roots(root, cfg));
    inputs
        .iter()
        .chain(dependencies)
        .any(|input| path.starts_with(input) || input.starts_with(path))
}

pub fn watch(
    root: PathBuf,
    drafts: bool,
    pdf: bool,
    jobs: Option<usize>,
    no_hooks: bool,
) -> Result<()> {
    let mut session = WatchSession::new(
        BuildOptions {
            root,
            drafts,
            pdf,
            jobs,
            force: false,
            keep_going: false,
            profile: false,
            explain: false,
            quiet: false,
            verbose: false,
        },
        no_hooks,
    )?;
    session.rebuild()?;
    println!("watching {}", session.opts.root.display());
    session.run(|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_waits_for_the_last_change() {
        let now = Instant::now();
        let mut pending = Pending::default();
        assert_eq!(pending.remaining(now), None);
        pending.changed(now);
        pending.changed(now + Duration::from_millis(200));
        assert_eq!(
            pending.remaining(now + Duration::from_millis(250)),
            Some(Duration::from_millis(200))
        );
        assert_eq!(
            pending.remaining(now + Duration::from_millis(450)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn watches_configs_missing_inputs_and_dependencies_but_not_outputs() {
        let root = Path::new("/project");
        let mut cfg = Config::default();
        let deps = BTreeSet::from([root.join("data/table.csv")]);
        for path in [
            "config.toml",
            "typage.plugins.toml",
            "content/new/page.typ",
            "data/table.csv",
        ] {
            assert!(relevant_path(root, &cfg, &deps, &root.join(path)));
        }
        for path in ["public/index.html", ".typage/cache.json", ".git/index"] {
            assert!(!relevant_path(root, &cfg, &deps, &root.join(path)));
        }
        cfg.content_dir = "articles".into();
        assert!(relevant_path(
            root,
            &cfg,
            &deps,
            &root.join("articles/new.typ")
        ));
        assert!(!relevant_path(
            root,
            &cfg,
            &deps,
            &root.join("content/old.typ")
        ));
    }
}
