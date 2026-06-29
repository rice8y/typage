use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
    pub updated: Option<String>,
    pub expires: Option<String>,
    pub weight: Option<i64>,
    pub lang: Option<String>,
    pub draft: bool,
    pub slug: Option<String>,
    /// Optional per-page permalink pattern. When omitted, `Config::permalink`
    /// or the legacy directory URL policy is used.
    pub permalink: Option<String>,
    pub template: Option<String>,
    pub section: Option<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub aliases: Vec<String>,
    pub build_pdf: Option<bool>,
    pub excerpt: Option<String>,
    pub toc: Option<bool>,
    pub sort_by: Option<String>,
    pub paginate_by: Option<usize>,
    pub extra: BTreeMap<String, toml::Value>,
    #[serde(flatten)]
    pub flattened_extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocItem {
    pub level: usize,
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub source: PathBuf,
    pub rel: PathBuf,
    pub body: String,
    pub processed_body: String,
    pub meta: FrontMatter,
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    pub slug: String,
    /// URL-ish content path without extension, e.g. `posts/hello`.
    pub path: String,
    /// Source file path relative to content_dir, e.g. `posts/hello.typ`.
    pub file_path: String,
    pub output_html: PathBuf,
    pub output_pdf: PathBuf,
    pub template: String,
    pub section: String,
    pub parent_section: Option<String>,
    pub ancestors: Vec<String>,
    pub toc: Vec<TocItem>,
    pub hash: String,
    pub prev_title: Option<String>,
    pub prev_url: Option<String>,
    pub next_title: Option<String>,
    pub next_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    pub kind: String,
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    pub slug: String,
    pub path: String,
    pub file_path: String,
    pub description: Option<String>,
    pub date: Option<String>,
    pub updated: Option<String>,
    pub weight: Option<i64>,
    pub section: String,
    pub parent_section: Option<String>,
    pub ancestors: Vec<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub aliases: Vec<String>,
    pub source: String,
    pub excerpt: Option<String>,
    pub toc: Vec<TocItem>,
    pub extra: BTreeMap<String, toml::Value>,
}

impl FrontMatter {
    pub fn extra(&self) -> BTreeMap<String, toml::Value> {
        let mut merged = self.flattened_extra.clone();
        for (key, value) in &self.extra {
            merged.insert(key.clone(), value.clone());
        }
        merged
    }
}

impl Page {
    pub fn summary(&self) -> PageSummary {
        PageSummary {
            kind: "page".to_string(),
            title: self.title.clone(),
            url: self.url.clone(),
            canonical_url: self.canonical_url.clone(),
            slug: self.slug.clone(),
            path: self.path.clone(),
            file_path: self.file_path.clone(),
            description: self.meta.description.clone(),
            date: self.meta.date.clone(),
            updated: self.meta.updated.clone(),
            weight: self.meta.weight,
            section: self.section.clone(),
            parent_section: self.parent_section.clone(),
            ancestors: self.ancestors.clone(),
            tags: self.meta.tags.clone(),
            categories: self.meta.categories.clone(),
            aliases: self.meta.aliases.clone(),
            source: self.rel.to_string_lossy().replace('\\', "/"),
            excerpt: self
                .meta
                .excerpt
                .clone()
                .or_else(|| self.meta.description.clone()),
            toc: self.toc.clone(),
            extra: self.meta.extra(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedPage {
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub url: String,
    pub output_html: PathBuf,
    pub items: Vec<PageSummary>,
    pub page_number: usize,
    pub total_pages: usize,
    pub prev_title: Option<String>,
    pub prev_url: Option<String>,
    pub next_title: Option<String>,
    pub next_url: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BuildCache {
    pub entries: std::collections::BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub hash: String,
    pub outputs: Vec<String>,
}

#[derive(Debug, Default)]
pub struct BuildStats {
    pub built: usize,
    pub skipped: usize,
    pub drafts: usize,
    pub future: usize,
    pub expired: usize,
    pub generated: usize,
    pub failed: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SkipStats {
    pub drafts: usize,
    pub future: usize,
    pub expired: usize,
}

impl SkipStats {
    pub fn total(self) -> usize {
        self.drafts + self.future + self.expired
    }
}
