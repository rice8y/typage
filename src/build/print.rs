//! Combined PDF selection, print wrappers, and compile-job planning.
use super::outputs::{
    pdf_document_label, pdf_document_output_path, pdf_document_output_url, pdf_document_public_path,
};
use super::{
    absolute_url, assign_prev_next, build_link_map, cache_decision, cached_template_hash,
    compare_pages_for_sort, dependency_hash, discover_pages, is_expired_date, is_future_date,
    page_dict, page_evaluation_context, page_include, preprocess_body, print_explain,
    template_hash, templates_root, BuildOptions, ContentCollections, PrintDocumentJob,
    TYPAGE_VERSION,
};
use crate::config::{Config, PdfDocumentConfig};
use crate::model::{BuildCache, BuildStats, FrontMatter, Page};
use crate::util::{
    hash_strs, relative_path, slugify, to_posix_path, typst_array_str, typst_opt_string,
    typst_string, typst_tuple, write_if_changed,
};
use anyhow::{bail, Result};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Copy)]
pub(super) struct PdfDocumentInclusion {
    drafts: bool,
    future: bool,
    expired: bool,
}

pub(super) fn pdf_document_inclusion(
    doc: &PdfDocumentConfig,
    cfg: &Config,
    drafts: bool,
) -> PdfDocumentInclusion {
    PdfDocumentInclusion {
        drafts: doc.include_drafts.unwrap_or(drafts),
        future: doc.include_future.unwrap_or(cfg.build_future),
        expired: doc.include_expired.unwrap_or(cfg.build_expired),
    }
}

/// Uses the union of document filters so discovery retains every possible input.
/// Selection reapplies each document's own filters to this shared candidate set.
pub(super) fn pdf_document_candidate_inclusion(cfg: &Config, drafts: bool) -> PdfDocumentInclusion {
    PdfDocumentInclusion {
        drafts: cfg
            .pdf_documents
            .iter()
            .any(|doc| doc.include_drafts.unwrap_or(drafts)),
        future: cfg
            .pdf_documents
            .iter()
            .any(|doc| doc.include_future.unwrap_or(cfg.build_future)),
        expired: cfg
            .pdf_documents
            .iter()
            .any(|doc| doc.include_expired.unwrap_or(cfg.build_expired)),
    }
}

pub(super) fn discover_print_candidate_pages(
    root: &Path,
    cfg: &Config,
    content_root: &Path,
    out_root: &Path,
    section_meta: &BTreeMap<String, FrontMatter>,
    collections: &ContentCollections,
    inclusion: PdfDocumentInclusion,
) -> Result<Vec<Page>> {
    let mut print_cfg = cfg.clone();
    print_cfg.build_future = inclusion.future;
    print_cfg.build_expired = inclusion.expired;
    let (mut pages, _) = discover_pages(
        root,
        &print_cfg,
        content_root,
        out_root,
        inclusion.drafts,
        collections,
    )?;
    assign_prev_next(&mut pages, section_meta);
    Ok(pages)
}

/// Selects explicit references first, preserving their configured order.
///
/// Sorted section matches follow, with duplicates removed by source path. Empty
/// selectors mean all eligible content pages, never generated list pages.
/// Unknown, ambiguous, excluded explicit references and empty results are errors.
pub(super) fn select_pdf_document_page_indices(
    doc: &PdfDocumentConfig,
    pages: &[Page],
    section_meta: &BTreeMap<String, FrontMatter>,
    inclusion: PdfDocumentInclusion,
) -> Result<Vec<usize>> {
    let mut selected = Vec::<usize>::new();
    let mut seen = BTreeSet::<String>::new();

    for reference in &doc.pages {
        let matches = pages
            .iter()
            .enumerate()
            .filter(|(_, page)| page_matches_pdf_reference(page, reference))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            bail!("unknown page reference {reference:?}");
        }
        if matches.len() > 1 {
            let candidates = matches
                .iter()
                .map(|(_, page)| page.file_path.clone())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("ambiguous page reference {reference:?}: {candidates}");
        }
        let (idx, page) = matches[0];
        if !page_allowed_in_pdf_document(page, inclusion) {
            bail!(
                "page reference {:?} is excluded by pdf document filters: {}",
                reference,
                page.source.display()
            );
        }
        insert_pdf_selection(&mut selected, &mut seen, idx, page);
    }

    let mut section_indices = Vec::<usize>::new();
    if !doc.sections.is_empty() {
        for section in &doc.sections {
            if !section_exists_for_pdf_document(section, pages, section_meta) {
                bail!("unknown section reference {section:?}");
            }
        }
        for (idx, page) in pages.iter().enumerate() {
            if !page_allowed_in_pdf_document(page, inclusion) {
                continue;
            }
            if doc
                .sections
                .iter()
                .any(|section| page_belongs_to_pdf_section(page, section))
            {
                section_indices.push(idx);
            }
        }
    } else if doc.pages.is_empty() {
        for (idx, page) in pages.iter().enumerate() {
            if page_allowed_in_pdf_document(page, inclusion) {
                section_indices.push(idx);
            }
        }
    }

    let sort_by = pdf_document_sort_by(doc, section_meta);
    section_indices.sort_by(|&a, &b| compare_pages_for_sort(&pages[a], &pages[b], &sort_by));
    for idx in section_indices {
        let page = &pages[idx];
        insert_pdf_selection(&mut selected, &mut seen, idx, page);
    }

    if selected.is_empty() {
        bail!(
            "pdf document {} selects no content pages",
            pdf_document_label(doc)
        );
    }
    Ok(selected)
}

fn insert_pdf_selection(
    selected: &mut Vec<usize>,
    seen: &mut BTreeSet<String>,
    idx: usize,
    page: &Page,
) {
    let key = page.rel.to_string_lossy().replace('\\', "/");
    if seen.insert(key) {
        selected.push(idx);
    }
}

fn pdf_document_sort_by(
    doc: &PdfDocumentConfig,
    section_meta: &BTreeMap<String, FrontMatter>,
) -> String {
    doc.sort_by
        .clone()
        .or_else(|| {
            (doc.sections.len() == 1)
                .then(|| {
                    section_meta
                        .get(&doc.sections[0])
                        .and_then(|meta| meta.sort_by.clone())
                })
                .flatten()
        })
        .unwrap_or_else(|| "date_desc".to_string())
}

fn page_allowed_in_pdf_document(page: &Page, inclusion: PdfDocumentInclusion) -> bool {
    if page.meta.draft && !inclusion.drafts {
        return false;
    }
    if is_future_date(page.meta.date.as_deref()) && !inclusion.future {
        return false;
    }
    if is_expired_date(page.meta.expires.as_deref()) && !inclusion.expired {
        return false;
    }
    true
}

fn section_exists_for_pdf_document(
    section: &str,
    pages: &[Page],
    section_meta: &BTreeMap<String, FrontMatter>,
) -> bool {
    section_meta.contains_key(section)
        || pages
            .iter()
            .any(|page| page_belongs_to_pdf_section(page, section))
}

fn page_belongs_to_pdf_section(page: &Page, section: &str) -> bool {
    page.section == section
        || page
            .section
            .strip_prefix(section)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn page_matches_pdf_reference(page: &Page, reference: &str) -> bool {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return false;
    }
    let without_anchor = trimmed.split('#').next().unwrap_or(trimmed);
    let mut candidates = BTreeSet::<String>::new();
    candidates.insert(page.url.clone());
    candidates.insert(page.url.trim_end_matches('/').to_string());
    candidates.insert(page.path.clone());
    candidates.insert(page.file_path.clone());
    candidates.insert(page.rel.to_string_lossy().replace('\\', "/"));
    candidates.insert(format!("@/{}", page.file_path));
    candidates.insert(format!("content/{}", page.file_path));
    let normalized = without_anchor.trim_matches('/').to_string();
    candidates.contains(without_anchor) || candidates.contains(&normalized)
}

pub(super) fn preprocess_print_pages(
    cfg: &Config,
    pages: &mut [Page],
    link_map: &BTreeMap<String, String>,
) -> Result<()> {
    for page in pages {
        let (toc, body, broken_links) =
            preprocess_body(&page.body, link_map, page.meta.toc.unwrap_or(true));
        if !broken_links.is_empty() {
            let msg = format!(
                "broken internal link(s) in {}: {}",
                page.source.display(),
                broken_links.join(", ")
            );
            if cfg.fail_on_broken_links {
                bail!(msg);
            } else {
                eprintln!("warning: {msg}");
            }
        }
        page.toc = toc;
        page.processed_body = body;
    }
    Ok(())
}

/// Immutable build inputs shared by all combined-PDF jobs.
/// Cache decisions and statistics stay separate from this source context.
pub(super) struct PrintDocumentContext<'a> {
    pub root: &'a Path,
    pub cfg: &'a Config,
    pub templates_root: &'a Path,
    pub out_root: &'a Path,
    pub cache_root: &'a Path,
    pub candidates: &'a [Page],
    pub section_meta: &'a BTreeMap<String, FrontMatter>,
    pub site_graph_hash: &'a str,
    pub cfg_hash_str: &'a str,
}

/// Writes wrappers for cache misses without compiling or publishing PDFs.
/// The caller owns compilation, dependency recording, and staged publication.
pub(super) fn prepare_print_document_jobs(
    context: PrintDocumentContext<'_>,
    cache: &BuildCache,
    template_hashes: &mut BTreeMap<String, String>,
    stats: &mut BuildStats,
    opts: &BuildOptions,
) -> Result<Vec<PrintDocumentJob>> {
    let PrintDocumentContext {
        root,
        cfg,
        templates_root,
        out_root,
        cache_root,
        candidates,
        section_meta,
        site_graph_hash,
        cfg_hash_str,
    } = context;
    if cfg.pdf_documents.is_empty() {
        return Ok(Vec::new());
    }

    let print_root = cache_root.join("print");
    crate::util::create_dir_all_checked(&print_root)?;
    let link_map = build_link_map(candidates);
    let mut jobs = Vec::<PrintDocumentJob>::new();

    for (idx, doc) in cfg.pdf_documents.iter().enumerate() {
        let public_path = pdf_document_public_path(doc)?;
        let output = pdf_document_output_path(out_root, doc)?;
        let inclusion = pdf_document_inclusion(doc, cfg, opts.drafts);
        let selected_indices =
            select_pdf_document_page_indices(doc, candidates, section_meta, inclusion)?;
        let mut selected_pages = selected_indices
            .iter()
            .map(|&idx| candidates[idx].clone())
            .collect::<Vec<_>>();
        preprocess_print_pages(cfg, &mut selected_pages, &link_map)?;

        let template_hash = cached_template_hash(template_hashes, templates_root, &doc.template)?;
        let document_json = serde_json::to_string(doc)?;
        let mut page_hashes = Vec::<String>::new();
        for page in &selected_pages {
            let meta_hash = serde_json::to_string(&page.meta)?;
            let dep_hash = dependency_hash(&page.source, &page.body)?;
            page_hashes.push(hash_strs(&[
                &page.processed_body,
                &meta_hash,
                &dep_hash,
                &page.url,
                &page.canonical_url,
                &page.template,
            ]));
        }
        let page_hash_refs = page_hashes.iter().map(String::as_str).collect::<Vec<_>>();
        // Preserve selection order in the hash: identical pages in a different
        // order still produce a different print document.
        let selected_hash = hash_strs(&page_hash_refs);
        let hash = hash_strs(&[
            &document_json,
            &selected_hash,
            site_graph_hash,
            cfg_hash_str,
            &template_hash,
        ]);
        let outputs = vec![output.clone()];
        let cache_key = format!("pdf_document:{public_path}");
        let label = format!("pdf document {public_path}");
        let decision = cache_decision(cache, &cache_key, &hash, &outputs, opts.force);
        if decision.hit {
            if opts.explain && !opts.quiet {
                print_explain("skip", &label, &decision.reasons);
            }
            stats.skipped += 1;
            continue;
        } else if opts.explain && !opts.quiet {
            print_explain("rebuild", &label, &decision.reasons);
        }

        let wrapper_path = print_document_wrapper_path(&print_root, idx, doc)?;
        let wrapper = print_document_wrapper_source(
            root,
            cfg,
            doc,
            &selected_pages,
            section_meta,
            &wrapper_path,
        )?;
        write_if_changed(&wrapper_path, &wrapper)?;
        let mut context = page_evaluation_context(&selected_pages);
        for page in &selected_pages {
            let body = if doc.section_headings.contains(&page.section) {
                shift_heading_levels(&page.processed_body, 1)
            } else {
                page.processed_body.clone()
            };
            // Keep the print-only page binding local to the source being included.
            context.sources.insert(
                page.source.clone(),
                format!("#let page = {}\n{body}", page_dict(page)),
            );
        }
        jobs.push(PrintDocumentJob {
            context,
            label,
            input: wrapper_path,
            output_pdf: output,
            current_json: pdf_document_current_json(cfg, doc, selected_pages.len())?,
            cache_key,
            hash,
            outputs,
        });
    }

    Ok(jobs)
}

fn print_document_wrapper_path(
    print_root: &Path,
    index: usize,
    doc: &PdfDocumentConfig,
) -> Result<PathBuf> {
    let public_path = pdf_document_public_path(doc)?;
    Ok(print_root.join(format!("{index}-{}.typ", slugify(&public_path))))
}

/// Places selected bodies in one Typst document using isolated content blocks.
/// Bodies are included at their original paths through compiler source overlays.
fn print_document_wrapper_source(
    root: &Path,
    cfg: &Config,
    doc: &PdfDocumentConfig,
    pages: &[Page],
    section_meta: &BTreeMap<String, FrontMatter>,
    wrapper_path: &Path,
) -> Result<String> {
    let wrapper_dir = wrapper_path.parent().unwrap_or(root);
    crate::util::create_dir_all_checked(wrapper_dir)?;
    let template_path = templates_root(root, cfg).join(&doc.template);
    let template_rel = relative_path(wrapper_dir, &template_path)?;
    let document = pdf_document_dict(cfg, doc, pages.len())?;
    let page_tuple = typst_tuple(pages.iter().map(page_dict).collect::<Vec<_>>());
    let mut out = format!(
        "#import {}: render\n#import \"@local/typage:{TYPAGE_VERSION}\" as typage\n\n#let __typage_document = {}\n#let __typage_pages = {}\n#set document(title: __typage_document.title)\n\n#show: render.with(site: typage.site, document: __typage_document, pages: __typage_pages)\n\n",
        typst_string(&to_posix_path(&template_rel)),
        document,
        page_tuple,
    );

    let section_heading_level = doc.section_heading_level.clamp(1, 5);
    let page_heading_level = (section_heading_level + 1).min(6);
    let section_heading_marks = heading_marks(section_heading_level);
    let page_heading_marks = heading_marks(page_heading_level);
    let mut last_heading_section: Option<&str> = None;

    for (idx, page) in pages.iter().enumerate() {
        let body = page_include(root, page)?;
        let has_section_heading = doc
            .section_headings
            .iter()
            .any(|section| section == &page.section);
        let title_heading_marks = if has_section_heading {
            &page_heading_marks
        } else {
            "="
        };
        out.push_str("#[\n");
        out.push_str(&format!("#let page = __typage_pages.at({idx})\n\n"));
        if has_section_heading && last_heading_section != Some(page.section.as_str()) {
            let title = pdf_document_section_heading_title(&page.section, section_meta);
            out.push_str(&format!(
                "{} #{}\n\n",
                section_heading_marks,
                typst_string(&title)
            ));
            last_heading_section = Some(page.section.as_str());
        }
        out.push_str(&format!("{title_heading_marks} #page.title\n\n"));
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("]\n");
        if idx + 1 < pages.len() {
            out.push_str("#pagebreak()\n\n");
        }
    }
    Ok(out)
}

fn heading_marks(level: usize) -> String {
    "=".repeat(level.clamp(1, 6))
}

fn pdf_document_section_heading_title(
    section: &str,
    section_meta: &BTreeMap<String, FrontMatter>,
) -> String {
    section_meta
        .get(section)
        .and_then(|meta| meta.title.clone())
        .unwrap_or_else(|| humanize_section_name(section))
}

fn humanize_section_name(section: &str) -> String {
    section
        .split(['/', '-', '_'])
        .filter(|part| !part.is_empty())
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize_first(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().collect::<String>() + chars.as_str()
}

fn shift_heading_levels(source: &str, amount: usize) -> String {
    if amount == 0 {
        return source.to_string();
    }

    let mut out = String::with_capacity(source.len());
    let mut in_raw_block = false;
    for line in source.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map(|body| (body, "\n"))
            .unwrap_or((line, ""));
        let raw_fence = body.trim_start().starts_with("```");
        if !in_raw_block && is_markup_heading_line(body) {
            out.push_str(&"=".repeat(amount));
        }
        out.push_str(body);
        out.push_str(newline);
        if raw_fence {
            in_raw_block = !in_raw_block;
        }
    }
    out
}

fn is_markup_heading_line(line: &str) -> bool {
    let marker_count = line.chars().take_while(|&ch| ch == '=').count();
    marker_count > 0 && line.chars().nth(marker_count) == Some(' ')
}

fn pdf_document_dict(cfg: &Config, doc: &PdfDocumentConfig, page_count: usize) -> Result<String> {
    let title = doc.title.clone().unwrap_or_else(|| cfg.title.clone());
    let path = pdf_document_public_path(doc)?;
    let url = pdf_document_output_url(doc)?;
    Ok(format!(
        "(kind: {}, title: {}, description: {}, path: {}, url: {}, canonical_url: {}, template: {}, sections: {}, source_pages: {}, section_headings: {}, section_heading_level: {}, page_count: {})",
        typst_string("pdf_document"),
        typst_string(&title),
        typst_opt_string(&doc.description),
        typst_string(&path),
        typst_string(&url),
        typst_string(&absolute_url(cfg, &url)),
        typst_string(&doc.template),
        typst_array_str(&doc.sections),
        typst_array_str(&doc.pages),
        typst_array_str(&doc.section_headings),
        doc.section_heading_level,
        page_count,
    ))
}

fn pdf_document_current_json(
    cfg: &Config,
    doc: &PdfDocumentConfig,
    page_count: usize,
) -> Result<String> {
    let title = doc.title.clone().unwrap_or_else(|| cfg.title.clone());
    let path = pdf_document_public_path(doc)?;
    let url = pdf_document_output_url(doc)?;
    Ok(serde_json::to_string(&json!({
        "kind": "pdf_document",
        "title": title,
        "description": &doc.description,
        "date": serde_json::Value::Null,
        "updated": serde_json::Value::Null,
        "weight": serde_json::Value::Null,
        "lang": &cfg.lang,
        "url": &url,
        "canonical_url": absolute_url(cfg, &url),
        "current_url": &url,
        "slug": serde_json::Value::Null,
        "path": path,
        "file_path": serde_json::Value::Null,
        "source": serde_json::Value::Null,
        "section": "pdf_document",
        "parent_section": serde_json::Value::Null,
        "ancestors": [],
        "tags": [],
        "categories": [],
        "aliases": [],
        "toc": [],
        "prev": serde_json::Value::Null,
        "next": serde_json::Value::Null,
        "template": &doc.template,
        "sections": &doc.sections,
        "source_pages": &doc.pages,
        "page_count": page_count,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::super::{parent_section_name, section_ancestors};
    use super::*;
    use std::ffi::OsStr;
    fn test_page(title: &str, file_path: &str, section: &str, date: &str) -> Page {
        let meta = FrontMatter {
            title: Some(title.to_string()),
            date: Some(date.to_string()),
            ..FrontMatter::default()
        };
        let path = file_path.trim_end_matches(".typ").to_string();
        let url = format!("/{path}/");
        Page {
            source: PathBuf::from("content").join(file_path),
            rel: PathBuf::from(file_path),
            body: String::new(),
            processed_body: String::new(),
            meta,
            title: title.to_string(),
            url: url.clone(),
            canonical_url: url,
            slug: Path::new(file_path)
                .file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or(title)
                .to_string(),
            path,
            file_path: file_path.to_string(),
            output_html: PathBuf::from("public")
                .join(file_path.trim_end_matches(".typ"))
                .join("index.html"),
            output_pdf: PathBuf::from("public")
                .join(file_path.trim_end_matches(".typ"))
                .join("index.pdf"),
            template: "base.typ".to_string(),
            section: section.to_string(),
            parent_section: parent_section_name(section),
            ancestors: section_ancestors(section),
            toc: Vec::new(),
            hash: String::new(),
            prev_title: None,
            prev_url: None,
            next_title: None,
            next_url: None,
        }
    }

    #[test]
    fn resolves_pdf_section_heading_titles() {
        let mut section_meta = BTreeMap::new();
        section_meta.insert(
            "blog".to_string(),
            FrontMatter {
                title: Some("Blog".to_string()),
                ..FrontMatter::default()
            },
        );

        assert_eq!(
            pdf_document_section_heading_title("blog", &section_meta),
            "Blog"
        );
        assert_eq!(
            pdf_document_section_heading_title("release-notes", &section_meta),
            "Release Notes"
        );
    }

    #[test]
    fn shifts_markup_headings_outside_raw_blocks() {
        let source = "intro\n== Section\n```typ\n== Raw\n```\n=== Nested\n";
        let got = shift_heading_levels(source, 1);
        assert_eq!(
            got,
            "intro\n=== Section\n```typ\n== Raw\n```\n==== Nested\n"
        );
    }

    #[test]
    fn selects_pdf_document_pages_with_explicit_order_then_sorted_sections() {
        let pages = vec![
            test_page("Old Project", "projects/old.typ", "projects", "2026-06-01"),
            test_page("New Post", "posts/new.typ", "posts", "2026-06-30"),
            test_page("Old Post", "posts/old.typ", "posts", "2026-06-01"),
        ];
        let doc = PdfDocumentConfig {
            path: "print.pdf".to_string(),
            pages: vec!["projects/old.typ".to_string()],
            sections: vec!["posts".to_string()],
            sort_by: Some("date_desc".to_string()),
            ..PdfDocumentConfig::default()
        };
        let indices = select_pdf_document_page_indices(
            &doc,
            &pages,
            &BTreeMap::new(),
            PdfDocumentInclusion {
                drafts: false,
                future: true,
                expired: true,
            },
        )
        .unwrap();
        let selected = indices
            .iter()
            .map(|&idx| pages[idx].file_path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            selected,
            vec!["projects/old.typ", "posts/new.typ", "posts/old.typ"]
        );
    }
}
