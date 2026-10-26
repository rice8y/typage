//! Output ownership, path validation, redirects, and stale-file cleanup.
use super::*;

pub(super) fn validate_routes(pages: &[Page], generated: &[GeneratedPage]) -> Result<()> {
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

pub(super) fn validate_pdf_document_configs(
    cfg: &Config,
    templates_root: &Path,
    out_root: &Path,
    pages: &[Page],
    section_meta: &BTreeMap<String, FrontMatter>,
    drafts: bool,
) -> Vec<String> {
    let mut errors = Vec::<String>::new();
    for (idx, doc) in cfg.pdf_documents.iter().enumerate() {
        let label = format!("pdf_documents[{idx}] ({})", pdf_document_label(doc));
        if let Err(err) = pdf_document_output_path(out_root, doc) {
            errors.push(format!("{label}: {err}"));
        }
        if doc.template.trim().is_empty() {
            errors.push(format!("{label}: template must not be empty"));
        } else if let Err(err) = template_hash(templates_root, &doc.template) {
            errors.push(format!("{label}: template error: {err:?}"));
        }
        if doc.sections.iter().any(|section| section.trim().is_empty()) {
            errors.push(format!("{label}: section references must not be empty"));
        }
        if doc.pages.iter().any(|page| page.trim().is_empty()) {
            errors.push(format!("{label}: page references must not be empty"));
        }
        let inclusion = pdf_document_inclusion(doc, cfg, drafts);
        if let Err(err) = select_pdf_document_page_indices(doc, pages, section_meta, inclusion) {
            errors.push(format!("{label}: {err}"));
        }
    }
    errors
}

pub(super) fn collect_reserved_output_paths(
    cfg: &Config,
    static_roots: &[PathBuf],
    out_root: &Path,
    pages: &[Page],
    generated: &[GeneratedPage],
    pdf: bool,
) -> Result<BTreeMap<PathBuf, String>> {
    let mut outputs = BTreeMap::<PathBuf, String>::new();
    for page in pages {
        insert_output_path(
            &mut outputs,
            clean_path(&page.output_html),
            format!("page {}", page.source.display()),
        )?;
        if pdf || cfg.build_pdf || page.meta.build_pdf.unwrap_or(false) {
            insert_output_path(
                &mut outputs,
                clean_path(&page.output_pdf),
                format!("page PDF {}", page.source.display()),
            )?;
        }
        for alias in &page.meta.aliases {
            insert_output_path(
                &mut outputs,
                output_path_for_url(out_root, &alias_to_url(alias)?)?,
                format!("alias {alias}"),
            )?;
        }
    }
    for gen in generated {
        insert_output_path(
            &mut outputs,
            clean_path(&gen.output_html),
            format!("generated {}", gen.title),
        )?;
    }
    for (rel, source) in static_assets(static_roots)? {
        insert_output_path(
            &mut outputs,
            clean_path(&out_root.join(rel)),
            format!("static asset {}", source.display()),
        )?;
    }
    Ok(outputs)
}

pub(super) fn static_assets(static_roots: &[PathBuf]) -> Result<BTreeMap<PathBuf, PathBuf>> {
    let mut assets = BTreeMap::new();
    for static_root in static_roots {
        if !static_root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(static_root).follow_root_links(false) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(static_root)?;
                assets.insert(rel.to_path_buf(), entry.path().to_path_buf());
            }
        }
    }
    Ok(assets)
}

pub(super) fn validate_output_paths(
    cfg: &Config,
    static_roots: &[PathBuf],
    out_root: &Path,
    pages: &[Page],
    generated: &[GeneratedPage],
    pdf: bool,
) -> Result<()> {
    let mut outputs =
        collect_reserved_output_paths(cfg, static_roots, out_root, pages, generated, pdf)?;
    let mut reserve =
        |path: PathBuf, label: String| insert_output_path(&mut outputs, clean_path(&path), label);
    for doc in &cfg.pdf_documents {
        reserve(
            pdf_document_output_path(out_root, doc)?,
            format!("pdf document {}", doc.path),
        )?;
    }
    if cfg.sitemap && !cfg.base_url.trim().is_empty() {
        reserve(out_root.join("sitemap.xml"), "sitemap".into())?;
    }
    if cfg.feed {
        reserve(
            public_file_path(out_root, &cfg.feed_path)?,
            "RSS feed".into(),
        )?;
        if cfg.atom_feed {
            reserve(
                public_file_path(out_root, &cfg.atom_path)?,
                "Atom feed".into(),
            )?;
        }
        for feed in &cfg.feeds {
            reserve(
                public_file_path(out_root, &feed.path)?,
                format!("feed {}", feed.path),
            )?;
        }
    }
    if cfg.robots {
        reserve(out_root.join("robots.txt"), "robots".into())?;
    }
    if cfg.search.enabled {
        reserve(out_root.join("search_index.json"), "search index".into())?;
    }
    for path in outputs.keys() {
        crate::util::check_output_ancestors(path)?;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.is_file() && !meta.file_type().is_symlink() {
                bail!("refusing to overwrite non-file output {}", path.display());
            }
        }
    }
    Ok(())
}

pub(super) fn insert_output_path(
    outputs: &mut BTreeMap<PathBuf, String>,
    path: PathBuf,
    label: String,
) -> Result<()> {
    let conflict = path
        .ancestors()
        .find_map(|ancestor| outputs.get_key_value(ancestor))
        .or_else(|| {
            outputs
                .range(path.clone()..)
                .next()
                .filter(|(existing, _)| existing.starts_with(&path))
        });
    if let Some((existing, prev)) = conflict {
        bail!(
            "output collision for {}: {} ({}) conflicts with {}",
            path.display(),
            prev,
            existing.display(),
            label
        );
    }
    outputs.insert(path, label);
    Ok(())
}

pub(super) fn insert_route(
    seen: &mut BTreeMap<String, String>,
    url: &str,
    label: String,
) -> Result<()> {
    if let Some(prev) = seen.insert(url.to_string(), label.clone()) {
        bail!("route collision for {url}: {prev} conflicts with {label}");
    }
    Ok(())
}

pub(super) fn validate_url_path(url: &str) -> Result<()> {
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

pub(super) fn alias_to_url(alias: &str) -> Result<String> {
    normalize_directory_url(alias).with_context(|| format!("invalid alias path: {alias}"))
}

pub(super) fn output_path_for_url(out_root: &Path, url: &str) -> Result<PathBuf> {
    validate_url_path(url)?;
    let clean = url.trim_matches('/');
    let path = if clean.is_empty() {
        out_root.join("index.html")
    } else {
        out_root.join(clean).join("index.html")
    };
    Ok(path)
}

pub(super) fn public_file_path(out_root: &Path, path: &str) -> Result<PathBuf> {
    let clean = public_file_url(path)?.trim_start_matches('/').to_string();
    Ok(out_root.join(clean))
}

pub(super) fn public_file_url(path: &str) -> Result<String> {
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

pub(super) fn pdf_document_public_path(doc: &PdfDocumentConfig) -> Result<String> {
    let trimmed = doc.path.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.contains('\\')
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.ends_with('/')
    {
        bail!("unsafe pdf document path: {trimmed}");
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        bail!("pdf document path must be relative: {trimmed}");
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => bail!("pdf document path must not contain traversal: {trimmed}"),
        }
    }
    if path.extension().and_then(OsStr::to_str) != Some("pdf") {
        bail!("pdf document path must end with .pdf: {trimmed}");
    }
    for segment in trimmed.split('/') {
        validate_url_segment(segment)?;
    }
    Ok(trimmed.to_string())
}

pub(super) fn pdf_document_output_path(
    out_root: &Path,
    doc: &PdfDocumentConfig,
) -> Result<PathBuf> {
    Ok(out_root.join(pdf_document_public_path(doc)?))
}

pub(super) fn pdf_document_output_url(doc: &PdfDocumentConfig) -> Result<String> {
    Ok(format!("/{}", pdf_document_public_path(doc)?))
}

pub(super) fn pdf_document_label(doc: &PdfDocumentConfig) -> String {
    if doc.path.trim().is_empty() {
        "<missing path>".to_string()
    } else {
        doc.path.clone()
    }
}

pub(super) fn write_alias_pages(out_root: &Path, pages: &[Page]) -> Result<()> {
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

pub(super) fn desired_public_outputs(
    cfg: &Config,
    static_roots: &[PathBuf],
    out_root: &Path,
    pages: &[Page],
    generated: &[GeneratedPage],
    pdf_documents: &[PathBuf],
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
    for pdf in pdf_documents {
        desired.insert(pdf.clone());
    }
    for relative in static_assets(static_roots)?.keys() {
        desired.insert(out_root.join(relative));
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

pub(super) fn cleanup_public_outputs(out_root: &Path, desired: &BTreeSet<PathBuf>) -> Result<()> {
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
