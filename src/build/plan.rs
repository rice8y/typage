//! Shared discovery and validation. Rendering starts only after this plan succeeds.
use super::*;

pub(super) struct SitePaths {
    pub content_root: PathBuf,
    pub templates_root: PathBuf,
    pub static_roots: Vec<PathBuf>,
    pub out_root: PathBuf,
}

impl SitePaths {
    pub fn new(root: &Path, cfg: &Config) -> Result<Self> {
        let paths = Self {
            content_root: root.join(&cfg.content_dir),
            templates_root: templates_root(root, cfg),
            static_roots: static_roots(root, cfg),
            out_root: root.join(&cfg.out_dir),
        };
        let errors = validate_directory_policy(
            root,
            cfg,
            &paths.content_root,
            &paths.templates_root,
            &paths.static_roots,
            &paths.out_root,
        );
        if !errors.is_empty() {
            bail!("invalid directory configuration\n\n{}", errors.join("\n"));
        }
        Ok(paths)
    }
}

pub(super) struct BuildPlan {
    pub paths: SitePaths,
    pub collections: ContentCollections,
    pub section_meta: BTreeMap<String, FrontMatter>,
    pub pages: Vec<Page>,
    pub summaries: Vec<PageSummary>,
    pub generated: Vec<GeneratedPage>,
    pub print_candidates: Vec<Page>,
    pub skipped: SkipStats,
}

impl BuildPlan {
    pub fn discover(root: &Path, cfg: &Config, drafts: bool, pdf: bool) -> Result<Self> {
        let paths = SitePaths::new(root, cfg)?;
        compiler::features(&cfg.features)?;
        compiler::features(&cfg.bundle_features)?;
        for (name, path) in [
            ("content", &paths.content_root),
            ("templates", &paths.templates_root),
        ] {
            if !path.is_dir() {
                bail!("missing {name} dir: {}", path.display());
            }
        }
        if let Some(theme) = cfg.theme.as_deref().filter(|name| !name.trim().is_empty()) {
            let report = inspect_theme(&root.join("themes").join(theme), Some(theme));
            if !report.errors.is_empty() {
                bail!("invalid theme\n{}", report.errors.join("\n"));
            }
        }
        let collections = load_content_collections(root, cfg, &paths.content_root)?;
        let section_meta = discover_sections(&paths.content_root, &collections)?;
        let (mut pages, skipped) = discover_pages(
            root,
            cfg,
            &paths.content_root,
            &paths.out_root,
            drafts,
            &collections,
        )?;
        assign_prev_next(&mut pages, &section_meta);
        let links = build_link_map(&pages);
        for page in &mut pages {
            let (toc, body, broken) =
                preprocess_body(&page.body, &links, page.meta.toc.unwrap_or(true));
            if !broken.is_empty() {
                let message = format!(
                    "broken internal link(s) in {}: {}",
                    page.source.display(),
                    broken.join(", ")
                );
                if cfg.fail_on_broken_links {
                    bail!(message);
                }
                eprintln!("warning: {message}");
            }
            page.toc = toc;
            page.processed_body = body;
            template_hash(&paths.templates_root, &page.template)
                .with_context(|| format!("template error for {}", page.source.display()))?;
        }
        let summaries = pages.iter().map(Page::summary).collect::<Vec<_>>();
        let generated = make_generated_pages(cfg, &paths.out_root, &summaries, &section_meta);
        if !generated.is_empty() {
            template_hash(&paths.templates_root, &cfg.list_template)?;
        }
        validate_routes(&pages, &generated)?;
        validate_output_paths(
            cfg,
            &paths.static_roots,
            &paths.out_root,
            &pages,
            &generated,
            pdf,
        )?;
        let print_candidates = if cfg.pdf_documents.is_empty() {
            Vec::new()
        } else {
            discover_print_candidate_pages(
                root,
                cfg,
                &paths.content_root,
                &paths.out_root,
                &section_meta,
                &collections,
                pdf_document_candidate_inclusion(cfg, drafts),
            )?
        };
        let errors = validate_pdf_document_configs(
            cfg,
            &paths.templates_root,
            &paths.out_root,
            &print_candidates,
            &section_meta,
            drafts,
        );
        if !errors.is_empty() {
            bail!(
                "invalid pdf document configuration\n\n{}",
                errors.join("\n")
            );
        }
        // Validate selected bodies here too, not only while preparing compile jobs.
        let print_links = build_link_map(&print_candidates);
        for doc in &cfg.pdf_documents {
            let indices = select_pdf_document_page_indices(
                doc,
                &print_candidates,
                &section_meta,
                pdf_document_inclusion(doc, cfg, drafts),
            )?;
            let mut selected = indices
                .into_iter()
                .map(|index| print_candidates[index].clone())
                .collect::<Vec<_>>();
            preprocess_print_pages(cfg, &mut selected, &print_links)?;
        }
        Ok(Self {
            paths,
            collections,
            section_meta,
            pages,
            summaries,
            generated,
            print_candidates,
            skipped,
        })
    }
}
