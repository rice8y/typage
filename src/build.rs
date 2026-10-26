//! Build orchestration and shared content/rendering support.
//!
//! Validate the site graph before preparing compile jobs. Compile into a staging
//! directory, then publish outputs and persist the cache only after success.
//! Domain-specific selection and serialization live in the child modules.

use self::feeds::{write_atom_feed, write_configured_feeds, write_robots, write_sitemap};
use self::outputs::{
    alias_to_url, cleanup_public_outputs, desired_public_outputs, pdf_document_output_path,
    static_assets, write_alias_pages,
};
use self::plan::{BuildPlan, SitePaths};
use self::print::{prepare_print_document_jobs, PrintDocumentContext};
use self::search::{search_diagnostics, write_search_index};
pub use self::theme::{list_themes, new_theme, theme_check, theme_info};
use self::theme::{
    read_theme_toml, static_roots, templates_root, theme_metadata_typ, validate_theme_name,
    ThemeToml,
};

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::compiler::{self, Compiler};
use crate::config::{load_config, Config};
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

mod feeds;
mod outputs;
mod pagination;
mod plan;
mod print;
mod search;
#[cfg(test)]
mod tests;
mod theme;

const TYPAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const INLINE_TYPAGE_CURRENT_LIMIT: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub root: PathBuf,
    pub drafts: bool,
    pub force: bool,
    pub pdf: bool,
    pub keep_going: bool,
    pub jobs: Option<usize>,
    pub profile: bool,
    pub explain: bool,
    pub quiet: bool,
    pub verbose: bool,
}

type ContentCollections = Arc<crate::metadata::MetadataContext>;

#[derive(Debug, Clone)]
struct CompileJob {
    context: compiler::EvaluationContext,
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
struct PrintDocumentJob {
    context: compiler::EvaluationContext,
    pages: Vec<(PathBuf, String)>,
    label: String,
    input: PathBuf,
    output_pdf: PathBuf,
    current_json: String,
    cache_key: String,
    hash: String,
    outputs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct CompileReport {
    label: String,
    duration: Duration,
    output_count: usize,
    generated: bool,
    dependencies: crate::compiler::Dependencies,
}

pub fn init_project(root: &Path) -> Result<()> {
    let root = normalize_path(root)?;
    crate::util::create_dir_all_checked(root.join("content"))?;
    crate::util::create_dir_all_checked(root.join("templates/components"))?;
    crate::util::create_dir_all_checked(root.join("static"))?;
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
        root.join("templates/print.typ"),
        include_str!("../templates/print.typ"),
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

pub fn doctor(root: PathBuf, drafts: bool) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    println!("root: {}", root.display());
    println!("config: {}", root.join("config.toml").display());
    println!("typst: embedded {}", compiler::VERSION);
    let plan = BuildPlan::discover(&root, &cfg, drafts, false)?;
    let errors = crate::plugin::diagnose_plugins(&root, &cfg);
    if !errors.is_empty() {
        bail!("invalid plugins\n{}", errors.join("\n"));
    }
    println!("content: {}", plan.paths.content_root.display());
    println!("templates: {}", plan.paths.templates_root.display());
    println!("out: {}", plan.paths.out_root.display());
    println!("pages: {}", plan.pages.len());
    println!("generated: {}", plan.generated.len());
    println!("pdf documents: {}", cfg.pdf_documents.len());
    println!("plugins: {}", cfg.plugins.len());
    println!(
        "skipped: drafts={} future={} expired={} total={}",
        plan.skipped.drafts,
        plan.skipped.future,
        plan.skipped.expired,
        plan.skipped.total()
    );
    for page in &plan.pages {
        if let (Some(date), Some(updated)) = (
            simple_date_prefix(page.meta.date.as_deref()),
            simple_date_prefix(page.meta.updated.as_deref()),
        ) {
            if updated < date {
                println!(
                    "warning: updated is before date in {}",
                    page.source.display()
                );
            }
        }
    }
    for warning in search_diagnostics(&cfg.search, &plan.pages) {
        println!("warning: {warning}");
    }
    if cfg.base_url.trim().is_empty() && cfg.sitemap {
        println!("warning: sitemap is skipped without base_url");
    }
    println!("doctor: ok");
    Ok(())
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
    BuildSession::default().build(opts)
}

/// Reuses expensive compiler resources across watch and live-reload builds.
///
/// Each compilation still reads sources through a fresh world. Configuration
/// changes or `--force` replace the shared compiler, including its font state.
#[derive(Default)]
pub struct BuildSession {
    compiler: Option<(String, Arc<Compiler>)>,
}

impl BuildSession {
    pub fn build(&mut self, opts: &BuildOptions) -> Result<BuildStats> {
        if opts.force {
            self.compiler = None;
        }
        let result = build_site_in_session(opts, self);
        comemo::evict(10);
        result
    }

    fn compiler(&mut self, root: &Path, cfg: &Config) -> Result<Arc<Compiler>> {
        let key = format!("{}:{}", root.display(), cfg_hash(cfg));
        if self
            .compiler
            .as_ref()
            .is_none_or(|(previous, _)| previous != &key)
        {
            self.compiler = Some((
                key,
                Arc::new(Compiler::new(
                    root,
                    root.join(&cfg.cache_dir).join("packages"),
                )?),
            ));
        }
        Ok(self.compiler.as_ref().unwrap().1.clone())
    }
}

fn build_site_in_session(opts: &BuildOptions, session: &mut BuildSession) -> Result<BuildStats> {
    let root = normalize_path(&opts.root)?;
    let cfg = load_config(&root)?;
    compiler::features(&cfg.features)?;
    let started = Instant::now();
    let cfg_hash_str = cfg_hash(&cfg);
    let mut template_hashes = BTreeMap::<String, String>::new();

    SitePaths::new(&root, &cfg)?;
    crate::plugin::run_build_plugins(&root, &cfg, opts.quiet)?;
    let BuildPlan {
        paths:
            SitePaths {
                content_root: _,
                templates_root,
                static_roots,
                out_root,
            },
        collections,
        section_meta,
        mut pages,
        summaries,
        generated,
        print_candidates,
        skipped,
    } = BuildPlan::discover(&root, &cfg, opts.drafts, opts.pdf)?;
    let cache_root = root.join(&cfg.cache_dir);
    let wrappers_root = cache_root.join("wrappers");
    let generated_root = cache_root.join("generated");
    let data_path = cache_root.join("site.typ");
    let cache_path = cache_root.join("build-cache.json");
    crate::util::create_dir_all_checked(&wrappers_root)?;
    crate::util::create_dir_all_checked(&generated_root)?;
    let mut cache = read_cache(&cache_path)?;
    let site_data = site_data_typ(&cfg, &summaries, &generated, &section_meta);
    let site_graph_hash = hash_strs(&[&site_data, &collections.hash()]);
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
        let wrapper = page_wrapper_source(&root, &cfg, &data_path, page, &wrapper_path)?;
        write_if_changed(&wrapper_path, &wrapper)?;
        compile_jobs.push(CompileJob {
            context: page_evaluation_context(std::slice::from_ref(page)),
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
            context: compiler::EvaluationContext::default(),
        });
    }

    let mut print_jobs = prepare_print_document_jobs(
        PrintDocumentContext {
            root: &root,
            cfg: &cfg,
            templates_root: &templates_root,
            out_root: &out_root,
            cache_root: &cache_root,
            candidates: &print_candidates,
            section_meta: &section_meta,
            site_graph_hash: &site_graph_hash,
            cfg_hash_str: &cfg_hash_str,
        },
        &cache,
        &mut template_hashes,
        &mut stats,
        opts,
    )?;
    let print_outputs = cfg
        .pdf_documents
        .iter()
        .map(|doc| pdf_document_output_path(&out_root, doc))
        .collect::<Result<Vec<_>>>()?;
    // Redirect compiler writes, but keep job.outputs pointing at public paths:
    // cache entries must never refer to this temporary staging directory.
    let staged = tempfile::Builder::new()
        .prefix("publish-")
        .tempdir_in(&cache_root)?;
    let stage_path =
        |path: &Path| -> Result<PathBuf> { Ok(staged.path().join(path.strip_prefix(&out_root)?)) };
    for job in &mut compile_jobs {
        job.html_output = stage_path(&job.html_output)?;
        job.pdf_output = job.pdf_output.as_deref().map(stage_path).transpose()?;
    }
    for job in &mut print_jobs {
        job.output_pdf = stage_path(&job.output_pdf)?;
    }
    let total_compile_jobs = compile_jobs.len() + print_jobs.len();
    let compile_concurrency = effective_jobs(opts.jobs, cfg.jobs, total_compile_jobs);
    if !opts.quiet {
        println!(
            "{} {} {}",
            term::bold("typage"),
            term::cyan("build"),
            term::dim(format!(
                "({total_compile_jobs} compile job{})",
                if total_compile_jobs == 1 { "" } else { "s" }
            ))
        );
    }
    let compile_started = Instant::now();
    let mut progress = BuildProgress::new(
        total_compile_jobs,
        !opts.quiet && !opts.verbose && total_compile_jobs > 0,
    );
    progress.draw();
    let compiler = session.compiler(&root, &cfg)?;
    compiler.set_metadata(collections.values(), collections.dependencies());
    let outcomes = run_compile_jobs(
        &cfg,
        &root,
        &compiler,
        compile_jobs,
        compile_concurrency,
        || progress.advance(),
    );
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
                        dependencies: Some(report.dependencies.clone()),
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
                    progress.finish();
                    return Err(err);
                }
            }
        }
    }

    for job in print_jobs {
        let result = run_print_document_job(&cfg, &root, &compiler, &job);
        progress.advance();
        match result {
            Ok(report) => {
                if !opts.quiet && opts.verbose {
                    println!(
                        "  {} {} {} {}",
                        term::green("✓"),
                        term::dim("pdf document"),
                        report.label,
                        term::dim(format_duration(report.duration))
                    );
                }
                cache.entries.insert(
                    job.cache_key.clone(),
                    CacheEntry {
                        hash: job.hash.clone(),
                        outputs: outputs_to_strings(&job.outputs),
                        dependencies: Some(report.dependencies.clone()),
                    },
                );
                stats.generated += 1;
                reports.push(report);
            }
            Err(err) => {
                stats.failed += 1;
                if opts.keep_going {
                    eprintln!("  {} {}", term::red("✗"), job.label);
                    failures.push(format!("{}\n{err:?}", job.label));
                } else {
                    progress.finish();
                    return Err(err);
                }
            }
        }
    }
    progress.finish();

    if !failures.is_empty() {
        bail!(
            "build failed; public outputs were not updated\n\n{}",
            failures.join("\n\n")
        );
    }

    for static_root in &static_roots {
        copy_dir(static_root, staged.path())?;
    }
    write_alias_pages(staged.path(), &pages)?;
    if cfg.sitemap {
        write_sitemap(&cfg, staged.path(), &summaries, &generated)?;
    }
    if cfg.feed {
        write_configured_feeds(&cfg, staged.path(), &summaries)?;
        if cfg.atom_feed {
            write_atom_feed(&cfg, staged.path(), &summaries)?;
        }
    }
    if cfg.robots {
        write_robots(&cfg, staged.path())?;
    }
    if cfg.search.enabled {
        write_search_index(staged.path(), &pages, &cfg.search)?;
    }
    let desired_outputs = desired_public_outputs(
        &cfg,
        &static_roots,
        &out_root,
        &pages,
        &generated,
        &print_outputs,
        opts.pdf,
    )?;
    // Publication is atomic per file, not for the entire output directory.
    // Compile failures leave the published site untouched; an I/O failure here
    // can partially publish files, but must not persist a successful cache.
    copy_dir(staged.path(), &out_root)?;
    cleanup_public_outputs(&out_root, &desired_outputs)?;
    write_cache(&cache_path, &cache)?;

    if !opts.quiet {
        print_build_summary(&stats, compile_concurrency, &out_root, started.elapsed());
        if opts.profile {
            print_profile(&reports, compile_wall);
        }
    }
    Ok(stats)
}

pub fn bundle_site(root: PathBuf, drafts: bool) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    SitePaths::new(&root, &cfg)?;
    crate::plugin::run_build_plugins(&root, &cfg, false)?;
    let BuildPlan {
        paths: SitePaths { out_root, .. },
        collections,
        section_meta,
        pages,
        summaries,
        generated,
        skipped,
        ..
    } = BuildPlan::discover(&root, &cfg, drafts, false)?;
    let cache_root = root.join(&cfg.cache_dir);
    crate::util::create_dir_all_checked(&cache_root)?;
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
        &bundle_source(&root, &cfg, &data_path, &pages, &generated)?,
    )?;
    let compiler = Compiler::new(&root, cache_root.join("packages"))?;
    compiler.set_metadata(collections.values(), collections.dependencies());
    let mut context = page_evaluation_context(&pages);
    let mut documents = typst::foundations::Array::new();
    let wrappers = cache_root.join("bundle-pages");
    for page in &pages {
        let wrapper_path = page_wrapper_path(&wrappers, page);
        write_if_changed(
            &wrapper_path,
            &page_wrapper_source(&root, &cfg, &data_path, page, &wrapper_path)?,
        )?;
        let inputs = current_input_args(
            &root,
            &cfg,
            &format!("bundle:{}", page.url),
            &page_current_json(page)?,
        )?;
        let (body, dependencies) = compiler.evaluate_content(
            &wrapper_path,
            &cfg.bundle_features,
            &input_arg_refs(&inputs),
            &page_evaluation_context(std::slice::from_ref(page)),
        )?;
        documents.push(typst::foundations::Value::Content(body));
        context.dependencies.extend(dependencies);
    }
    for gen in &generated {
        let wrapper_path = cache_root
            .join("generated")
            .join(format!("{}.typ", slugify(&gen.url)));
        write_if_changed(
            &wrapper_path,
            &generated_wrapper_source(&root, &cfg, &data_path, gen)?,
        )?;
        let inputs = current_input_args(
            &root,
            &cfg,
            &format!("bundle:{}", gen.url),
            &generated_current_json(gen, &cfg)?,
        )?;
        let (body, dependencies) = compiler.evaluate_content(
            &wrapper_path,
            &cfg.bundle_features,
            &input_arg_refs(&inputs),
            &compiler::EvaluationContext::default(),
        )?;
        documents.push(typst::foundations::Value::Content(body));
        context.dependencies.extend(dependencies);
    }
    context.bindings.insert(
        "__typage_documents".into(),
        typst::foundations::Value::Array(documents),
    );
    let staged = tempfile::Builder::new()
        .prefix("bundle-")
        .tempdir_in(&cache_root)?;
    compile_typst(
        &compiler,
        compiler::CompileRequest {
            input: &bundle_path,
            output: staged.path(),
            format: "bundle",
            features: &cfg.bundle_features,
            inputs: &[],
        },
        &context,
        "bundle",
    )?;
    copy_dir(staged.path(), &out_root)?;
    println!("bundle output: {}", out_root.display());
    Ok(())
}

fn discover_sections(
    content_root: &Path,
    collections: &ContentCollections,
) -> Result<BTreeMap<String, FrontMatter>> {
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
        let (meta, _) = collections
            .parse(path, &raw, "")
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
        parts.push(format!(
            "{}:{}",
            canonical.display(),
            crate::compiler::fingerprint(&bytes)
        ));
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
        let (mut meta, body) = collections
            .parse(path, &raw, &infer_section(path.strip_prefix(content_root)?))
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

fn load_content_collections(
    root: &Path,
    cfg: &Config,
    content_root: &Path,
) -> Result<ContentCollections> {
    Ok(Arc::new(crate::metadata::MetadataContext::new(
        root,
        content_root,
        root.join(&cfg.cache_dir).join("packages"),
    )?))
}

#[cfg(test)]
fn split_metadata(path: &Path, raw: &str) -> Result<(FrontMatter, String)> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let path = root.join(path);
    crate::util::create_dir_all_checked(path.parent().unwrap())?;
    fs::write(&path, raw)?;
    let context =
        crate::metadata::MetadataContext::new(&root, &root.join("content"), root.join("packages"))?;
    context.parse(&path, raw, "")
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
    s.replace(['`', '*', '_'], "").trim().to_string()
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

/// Requires matching inputs, unchanged compiler reads, and regular output files.
///
/// Missing dependency manifests force a rebuild of older cache entries. Keep
/// all miss reasons so `--explain` can describe more than the first difference.
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
    if entry.outputs != outputs_to_strings(outputs) {
        reasons.push("output set changed".to_string());
    }
    match &entry.dependencies {
        Some(dependencies) => {
            for path in crate::compiler::changed_dependencies(dependencies) {
                reasons.push(format!("dependency changed: {}", path.display()));
            }
        }
        None => reasons.push("dependency manifest missing".to_string()),
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

struct BuildProgress {
    enabled: bool,
    total: usize,
    done: usize,
    started: Instant,
    last_draw: Instant,
    last_len: usize,
}

impl BuildProgress {
    fn new(total: usize, enabled: bool) -> Self {
        let now = Instant::now();
        Self {
            enabled,
            total,
            done: 0,
            started: now,
            last_draw: now,
            last_len: 0,
        }
    }

    fn advance(&mut self) {
        if self.total > 0 {
            self.done = (self.done + 1).min(self.total);
        }
        if self.enabled
            && (self.done == self.total || self.last_draw.elapsed() >= Duration::from_millis(100))
        {
            self.draw();
        }
    }

    fn draw(&mut self) {
        if !self.enabled {
            return;
        }
        let width = 24usize;
        let filled = if self.total == 0 {
            width
        } else {
            self.done * width / self.total
        };
        let percent = if self.total == 0 {
            100
        } else {
            self.done * 100 / self.total
        };
        let bar = format!("{}{}", "=".repeat(filled), " ".repeat(width - filled));
        let line = format!(
            "  {} [{}] {}/{} {} {}",
            term::cyan("compile"),
            bar,
            self.done,
            self.total,
            term::dim(format!("{percent:>3}%")),
            term::dim(format_duration(self.started.elapsed()))
        );
        let padding = " ".repeat(self.last_len.saturating_sub(line.len()));
        print!("\r{line}{padding}");
        let _ = io::stdout().flush();
        self.last_len = line.len();
        self.last_draw = Instant::now();
    }

    fn finish(&mut self) {
        if self.enabled {
            if self.done < self.total || self.last_len == 0 {
                self.draw();
            }
            println!();
            self.enabled = false;
        }
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
    compiler: &Arc<Compiler>,
    jobs: Vec<CompileJob>,
    concurrency: usize,
    mut on_complete: impl FnMut(),
) -> Vec<(CompileJob, Result<CompileReport>)> {
    if jobs.is_empty() {
        return Vec::new();
    }
    if concurrency <= 1 {
        let mut outcomes = Vec::with_capacity(jobs.len());
        for job in jobs {
            let result = run_compile_job(cfg, root, compiler, &job);
            on_complete();
            outcomes.push((job, result));
        }
        return outcomes;
    }

    let (job_tx, job_rx) = mpsc::channel::<CompileJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (out_tx, out_rx) = mpsc::channel::<(CompileJob, Result<CompileReport>)>();

    for _ in 0..concurrency {
        let job_rx = Arc::clone(&job_rx);
        let out_tx = out_tx.clone();
        let cfg = cfg.clone();
        let root = root.to_path_buf();
        let compiler = Arc::clone(compiler);
        thread::spawn(move || loop {
            let next = { job_rx.lock().expect("compile job receiver poisoned").recv() };
            let Ok(job) = next else {
                break;
            };
            let result = run_compile_job(&cfg, &root, &compiler, &job);
            let _ = out_tx.send((job, result));
        });
    }
    drop(out_tx);

    for job in jobs {
        let _ = job_tx.send(job);
    }
    drop(job_tx);

    let mut outcomes = Vec::new();
    for outcome in out_rx {
        on_complete();
        outcomes.push(outcome);
    }
    outcomes
}

fn current_input_args(
    root: &Path,
    cfg: &Config,
    cache_key: &str,
    current_json: &str,
) -> Result<Vec<(String, String)>> {
    if current_json.len() <= INLINE_TYPAGE_CURRENT_LIMIT {
        return Ok(vec![(
            "typage_current".to_string(),
            current_json.to_string(),
        )]);
    }

    let package_root = typage_package_root(root, cfg);
    let current_dir = package_root.join("current");
    crate::util::create_dir_all_checked(&current_dir)?;
    let filename = format!("{}-{}.json", slugify(cache_key), hash_strs(&[current_json]));
    let path = current_dir.join(filename);
    write_if_changed(&path, current_json)?;
    let current_path = relative_path(&package_root, &path)?;
    Ok(vec![(
        "typage_current_file".to_string(),
        to_posix_path(&current_path),
    )])
}

fn input_arg_refs(inputs: &[(String, String)]) -> Vec<(&str, &str)> {
    inputs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

fn run_compile_job(
    cfg: &Config,
    root: &Path,
    compiler: &Compiler,
    job: &CompileJob,
) -> Result<CompileReport> {
    let started = Instant::now();
    let inputs = current_input_args(root, cfg, &job.cache_key, &job.current_json)?;
    let input_refs = input_arg_refs(&inputs);
    let mut dependencies = compile_typst(
        compiler,
        compiler::CompileRequest {
            input: &job.input,
            output: &job.html_output,
            format: "html",
            features: &job.html_features,
            inputs: &input_refs,
        },
        &job.context,
        &job.label,
    )?;
    if let Some(pdf_output) = &job.pdf_output {
        dependencies.extend(compile_typst(
            compiler,
            compiler::CompileRequest {
                input: &job.input,
                output: pdf_output,
                format: "pdf",
                features: "",
                inputs: &input_refs,
            },
            &job.context,
            &format!("{} [pdf]", job.label),
        )?);
    }
    Ok(CompileReport {
        label: job.label.clone(),
        duration: started.elapsed(),
        output_count: job.outputs.len(),
        generated: job.generated,
        dependencies,
    })
}

fn run_print_document_job(
    cfg: &Config,
    root: &Path,
    compiler: &Compiler,
    job: &PrintDocumentJob,
) -> Result<CompileReport> {
    let started = Instant::now();
    let inputs = current_input_args(root, cfg, &job.cache_key, &job.current_json)?;
    let input_refs = input_arg_refs(&inputs);
    let mut context = job.context.clone();
    let mut bodies = typst::foundations::Array::new();
    for (index, (source, current_json)) in job.pages.iter().enumerate() {
        let inputs = current_input_args(
            root,
            cfg,
            &format!("{}:{index}", job.cache_key),
            current_json,
        )?;
        let (body, dependencies) = compiler
            .evaluate_content(source, "", &input_arg_refs(&inputs), &job.context)
            .with_context(|| format!("failed to evaluate print page {}", source.display()))?;
        bodies.push(typst::foundations::Value::Content(body));
        context.dependencies.extend(dependencies);
    }
    context.bindings.insert(
        "__typage_bodies".into(),
        typst::foundations::Value::Array(bodies),
    );
    let dependencies = compile_typst(
        compiler,
        compiler::CompileRequest {
            input: &job.input,
            output: &job.output_pdf,
            format: "pdf",
            features: "",
            inputs: &input_refs,
        },
        &context,
        &job.label,
    )?;
    Ok(CompileReport {
        label: job.label.clone(),
        duration: started.elapsed(),
        output_count: job.outputs.len(),
        generated: true,
        dependencies,
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
            object.insert(name.clone(), field.current_marker());
        }
    }
    Ok(serde_json::to_string(&value)?)
}

fn generated_current_json(gen: &GeneratedPage, cfg: &Config) -> Result<String> {
    let items = gen
        .items
        .iter()
        .map(page_summary_json)
        .collect::<Result<Vec<_>>>()?;
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
        "items": items,
        "page_number": gen.page_number,
        "total_pages": gen.total_pages,
    }))?)
}

fn page_summary_json(page: &PageSummary) -> Result<serde_json::Value> {
    let mut value = json!({
        "kind": &page.kind,
        "title": &page.title,
        "description": &page.description,
        "excerpt": &page.excerpt,
        "date": &page.date,
        "updated": &page.updated,
        "weight": page.weight,
        "url": &page.url,
        "canonical_url": &page.canonical_url,
        "current_url": &page.url,
        "slug": &page.slug,
        "path": &page.path,
        "file_path": &page.file_path,
        "source": &page.source,
        "section": &page.section,
        "parent_section": &page.parent_section,
        "ancestors": &page.ancestors,
        "tags": &page.tags,
        "categories": &page.categories,
        "aliases": &page.aliases,
        "toc": &page.toc,
    });
    if let Some(object) = value.as_object_mut() {
        for (name, field) in &page.fields {
            object.insert(name.clone(), field.current_marker());
        }
    }
    Ok(value)
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
    let env = [
        "TYPST_FONT_PATHS",
        "TYPST_IGNORE_SYSTEM_FONTS",
        "TYPST_IGNORE_EMBEDDED_FONTS",
        "TYPST_PACKAGE_PATH",
        "TYPST_PACKAGE_CACHE_PATH",
        "SOURCE_DATE_EPOCH",
    ]
    .map(|key| {
        (
            key,
            std::env::var_os(key).map(|value| value.to_string_lossy().into_owned()),
        )
    });
    serde_json::to_string(&(
        cfg,
        TYPAGE_VERSION,
        "source-context-v2",
        compiler::VERSION,
        env,
    ))
    .unwrap_or_default()
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
) -> Result<String> {
    let wrapper_dir = wrapper_path.parent().unwrap_or(root);
    crate::util::create_dir_all_checked(wrapper_dir)?;
    let template_path = templates_root(root, cfg).join(&page.template);
    let template_rel = relative_path(wrapper_dir, &template_path)?;
    let body = page_include(root, page)?;
    let current = page_dict(page);
    Ok(format!(
        "#import {}: render\n#import \"@local/typage:{TYPAGE_VERSION}\" as typage\n\n#let __typage_current = {}\n#show: render.with(site: typage.site, page: __typage_current, pages: typage.pages, taxonomies: typage.taxonomies)\n\n{}\n",
        typst_string(&to_posix_path(&template_rel)),
        current,
        body
    ))
}

fn page_evaluation_context(pages: &[Page]) -> compiler::EvaluationContext {
    compiler::EvaluationContext {
        sources: pages
            .iter()
            .map(|page| (page.source.clone(), page.processed_body.clone().into()))
            .collect(),
        ..Default::default()
    }
}

fn page_include(root: &Path, page: &Page) -> Result<String> {
    let path = page.source.strip_prefix(root).with_context(|| {
        format!(
            "content source is outside the project: {}",
            page.source.display()
        )
    })?;
    Ok(format!(
        "#include {}\n",
        typst_string(&format!("/{}", to_posix_path(path)))
    ))
}

fn generated_wrapper_source(
    root: &Path,
    cfg: &Config,
    _data_path: &Path,
    gen: &GeneratedPage,
) -> Result<String> {
    let wrapper_dir = root.join(&cfg.cache_dir).join("generated");
    crate::util::create_dir_all_checked(&wrapper_dir)?;
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
        "(kind: \"page\", title: {}, description: {}, excerpt: {}, date: {}, updated: {}, weight: {}, lang: {}, url: {}, canonical_url: {}, current_url: {}, slug: {}, path: {}, file_path: {}, source: {}, section: {}, parent_section: {}, ancestors: {}, tags: {}, categories: {}, aliases: {}, toc: {}, prev: {}, next: {}){}",
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

fn summary_dict(page: &PageSummary) -> String {
    format!(
        "(kind: {}, title: {}, url: {}, canonical_url: {}, slug: {}, path: {}, file_path: {}, description: {}, date: {}, updated: {}, weight: {}, section: {}, parent_section: {}, ancestors: {}, tags: {}, categories: {}, aliases: {}, source: {}, excerpt: {}, toc: {}){}",
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
        .map(|(name, field)| format!("{}: {}", typst_string(name), field.expression()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" + ({fields})")
}

fn typst_ident(key: &str) -> String {
    let mut out = String::new();
    for (i, ch) in key.chars().enumerate() {
        if ch == '_' || ch.is_alphabetic() || (i > 0 && ch.is_ascii_digit()) {
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
            "(name: {}, title: {}, description: {}, path: {}, parent: {}, ancestors: {}, children: {}, pages: {}, sort_by: {}, weight: {}){}",
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
    crate::util::create_dir_all_checked(&pkg_root)?;
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
    let lib = include_str!("runtime.typ");
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
    crate::util::create_dir_all_checked(&pkg_root)?;
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
    compiler: &Compiler,
    request: compiler::CompileRequest<'_>,
    context: &compiler::EvaluationContext,
    label: &str,
) -> Result<crate::compiler::Dependencies> {
    let input = request.input;
    let output = request.output;
    compiler
        .compile_with_context(request, context)
        .with_context(|| {
            format!(
                "Typst compilation failed for {label}\ninput: {}\noutput: {}",
                input.display(),
                output.display()
            )
        })
}

fn bundle_source(
    root: &Path,
    cfg: &Config,
    _data_path: &Path,
    pages: &[Page],
    generated: &[GeneratedPage],
) -> Result<String> {
    let bundle_dir = root.join(&cfg.cache_dir);
    let mut out = String::new();
    out.push_str("// Generated by typage. Experimental Typst bundle export.\n");
    // Bodies and templates have already captured their own current-page values.
    for (index, url) in pages
        .iter()
        .map(|page| &page.url)
        .chain(generated.iter().map(|page| &page.url))
        .enumerate()
    {
        let out_path = url.trim_start_matches('/').trim_end_matches('/');
        let html_path = if out_path.is_empty() {
            "index.html".to_string()
        } else {
            format!("{out_path}/index.html")
        };
        out.push_str(&format!(
            "#document({})[#__typage_documents.at({index})]\n",
            typst_string(&html_path)
        ));
    }
    for (relative, source) in static_assets(&static_roots(root, cfg))? {
        let src_rel = relative_path(&bundle_dir, &source)?;
        out.push_str(&format!(
            "#asset({}, read({}, encoding: none))\n",
            typst_string(&to_posix_path(&relative)),
            typst_string(&to_posix_path(&src_rel))
        ));
    }
    Ok(out)
}
