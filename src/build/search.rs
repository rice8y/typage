//! Search index construction, text extraction, and tokenization.
use crate::config::SearchConfig;
use crate::model::Page;
use crate::util::write_if_changed;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Serialize)]
struct SearchIndex {
    version: u32,
    config: SearchIndexConfig,
    pages: Vec<SearchEntry>,
}

#[derive(Serialize)]
struct SearchIndexConfig {
    mode: String,
    ngram: usize,
    compact: bool,
}

#[derive(Serialize)]
struct SearchEntry {
    title: String,
    url: String,
    description: Option<String>,
    date: Option<String>,
    updated: Option<String>,
    weight: Option<i64>,
    section: String,
    tags: Vec<String>,
    categories: Vec<String>,
    excerpt: String,
    /// Full body text. Empty when `search.compact = true`.
    text: String,
    tokens: Vec<String>,
    headings: Vec<SearchHeading>,
    fields: SearchFields,
}

#[derive(Serialize)]
struct SearchHeading {
    level: usize,
    id: String,
    text: String,
    tokens: Vec<String>,
}

#[derive(Serialize)]
struct SearchFields {
    title: Vec<String>,
    headings: Vec<String>,
    body: Vec<String>,
    taxonomies: Vec<String>,
}

pub(super) fn search_diagnostics(cfg: &SearchConfig, pages: &[Page]) -> Vec<String> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let mode = cfg.mode.as_str();
    if !matches!(mode, "auto" | "latin" | "cjk" | "ngram" | "ngram-only") {
        warnings.push(format!(
            "unknown search.mode `{}`; expected auto, latin, cjk, ngram, or ngram-only",
            cfg.mode
        ));
    }
    if cfg.ngram == 0 || cfg.ngram > 8 {
        warnings.push(format!(
            "search.ngram={} will be clamped to {}",
            cfg.ngram,
            normalized_ngram(cfg)
        ));
    }
    let mut indexed_chars = 0usize;
    let mut cjk_pages = 0usize;
    for page in pages {
        let text = plain_text_for_search(&page.processed_body);
        indexed_chars += page.title.chars().count();
        indexed_chars += page
            .meta
            .description
            .as_deref()
            .unwrap_or("")
            .chars()
            .count();
        indexed_chars += text.chars().count();
        if text.chars().any(is_cjk) || page.title.chars().any(is_cjk) {
            cjk_pages += 1;
        }
    }
    println!(
        "search: {} mode={} ngram={} compact={} pages={} cjk_pages={} indexed_chars≈{}",
        if cfg.enabled { "enabled" } else { "disabled" },
        cfg.mode,
        normalized_ngram(cfg),
        cfg.compact,
        pages.len(),
        cjk_pages,
        indexed_chars
    );
    if cjk_pages > 0 && matches!(mode, "latin") {
        warnings.push(
            "CJK text detected but search.mode = \"latin\"; consider mode = \"cjk\" or \"auto\""
                .to_string(),
        );
    }
    if !cfg.compact && indexed_chars > 500_000 {
        warnings.push(format!(
            "search index may be large ({} indexed chars); consider [search] compact = true",
            indexed_chars
        ));
    }
    warnings
}

/// Writes a source-derived index without compiling or inspecting rendered HTML.
/// Field token sets support client-side ranking; compact mode omits full text.
pub(super) fn write_search_index(
    out_root: &Path,
    pages: &[Page],
    cfg: &SearchConfig,
) -> Result<()> {
    let entries = pages
        .iter()
        .map(|page| {
            let text = plain_text_for_search(&page.processed_body);
            let excerpt = page
                .meta
                .excerpt
                .clone()
                .or_else(|| page.meta.description.clone())
                .unwrap_or_else(|| make_excerpt(&text, 180));
            let title_tokens = tokenize_search_text(&page.title, cfg);
            let heading_entries = if cfg.include_headings {
                page.toc
                    .iter()
                    .map(|item| SearchHeading {
                        level: item.level,
                        id: item.id.clone(),
                        text: truncate_chars(&item.text, cfg.max_heading_chars),
                        tokens: tokenize_search_text(&item.text, cfg),
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let heading_tokens = dedupe_tokens(
                heading_entries
                    .iter()
                    .flat_map(|h| h.tokens.iter().cloned())
                    .collect::<Vec<_>>(),
                cfg.max_tokens,
            );
            let body_tokens = if cfg.include_body {
                tokenize_search_text(&text, cfg)
            } else {
                Vec::new()
            };
            let taxonomy_text = taxonomy_search_text(page, cfg);
            let taxonomy_tokens = tokenize_search_text(&taxonomy_text, cfg);
            let mut all_tokens = Vec::new();
            all_tokens.extend(title_tokens.clone());
            all_tokens.extend(tokenize_search_text(
                page.meta.description.as_deref().unwrap_or(""),
                cfg,
            ));
            all_tokens.extend(tokenize_search_text(&excerpt, cfg));
            all_tokens.extend(heading_tokens.clone());
            all_tokens.extend(body_tokens.clone());
            all_tokens.extend(taxonomy_tokens.clone());
            let tokens = dedupe_tokens(all_tokens, cfg.max_tokens);
            SearchEntry {
                title: page.title.clone(),
                url: page.url.clone(),
                description: page.meta.description.clone(),
                date: page.meta.date.clone(),
                updated: page.meta.updated.clone(),
                weight: page.meta.weight,
                section: page.section.clone(),
                tags: page.meta.tags.clone(),
                categories: page.meta.categories.clone(),
                excerpt,
                text: if cfg.compact {
                    String::new()
                } else {
                    truncate_chars(&text, cfg.max_body_chars)
                },
                tokens,
                headings: heading_entries,
                fields: SearchFields {
                    title: title_tokens,
                    headings: heading_tokens,
                    body: body_tokens,
                    taxonomies: taxonomy_tokens,
                },
            }
        })
        .collect::<Vec<_>>();
    let index = SearchIndex {
        version: 2,
        config: SearchIndexConfig {
            mode: cfg.mode.clone(),
            ngram: normalized_ngram(cfg),
            compact: cfg.compact,
        },
        pages: entries,
    };
    let json = serde_json::to_string_pretty(&index)?;
    write_if_changed(&out_root.join("search_index.json"), &json)
}

fn taxonomy_search_text(page: &Page, cfg: &SearchConfig) -> String {
    if !cfg.include_tags && !cfg.include_taxonomies {
        return String::new();
    }
    let mut parts = Vec::new();
    if cfg.include_tags {
        parts.extend(page.meta.tags.iter().cloned());
        parts.extend(page.meta.categories.iter().cloned());
    }
    if cfg.include_taxonomies {
        for (key, field) in &page.meta.fields {
            if matches!(key.as_str(), "tags" | "categories") {
                continue;
            }
            let Some(value) = field.export_toml() else {
                continue;
            };
            match value {
                toml::Value::String(s) => parts.push(s),
                toml::Value::Array(xs) => {
                    for x in xs {
                        if let Some(s) = x.as_str() {
                            parts.push(s.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    parts.join(" ")
}

#[cfg(test)]
fn search_tokens(page: &Page, text: &str, excerpt: &str) -> Vec<String> {
    let cfg = SearchConfig::default();
    let mut all = Vec::new();
    all.extend(tokenize_search_text(&page.title, &cfg));
    all.extend(tokenize_search_text(
        page.meta.description.as_deref().unwrap_or(""),
        &cfg,
    ));
    all.extend(tokenize_search_text(excerpt, &cfg));
    all.extend(tokenize_search_text(text, &cfg));
    all.extend(tokenize_search_text(
        &taxonomy_search_text(page, &cfg),
        &cfg,
    ));
    dedupe_tokens(all, cfg.max_tokens)
}

// Flush when scripts change so word tokens and CJK n-grams never span a boundary.
fn tokenize_search_text(text: &str, cfg: &SearchConfig) -> Vec<String> {
    let mode = cfg.mode.as_str();
    let include_latin = mode != "ngram-only";
    let include_cjk = mode == "auto" || mode == "cjk" || mode == "ngram" || mode == "ngram-only";
    let mut tokens = Vec::<String>::new();
    let mut latin = String::new();
    let mut cjk = String::new();

    for ch in text.chars() {
        if is_cjk(ch) {
            push_latin_token_vec(&mut tokens, &mut latin);
            if include_cjk {
                cjk.push(ch);
            }
        } else if ch.is_alphanumeric() || ch == '_' {
            push_cjk_ngrams_vec(&mut tokens, &mut cjk, cfg);
            if include_latin {
                for lower in ch.to_lowercase() {
                    latin.push(lower);
                }
            }
        } else {
            push_latin_token_vec(&mut tokens, &mut latin);
            push_cjk_ngrams_vec(&mut tokens, &mut cjk, cfg);
        }
    }
    push_latin_token_vec(&mut tokens, &mut latin);
    push_cjk_ngrams_vec(&mut tokens, &mut cjk, cfg);
    dedupe_tokens(tokens, cfg.max_tokens)
}

// Preserve first-seen order before truncation; sorting would change which tokens
// survive the limit and could discard earlier, higher-priority fields.
fn dedupe_tokens(tokens: Vec<String>, max_tokens: usize) -> Vec<String> {
    let mut set = BTreeSet::new();
    let mut out = Vec::new();
    let limit = max_tokens.max(1);
    for token in tokens {
        if token.is_empty() {
            continue;
        }
        if set.insert(token.clone()) {
            out.push(token);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn push_latin_token_vec(out: &mut Vec<String>, current: &mut String) {
    let token = current.trim_matches('_').to_string();
    current.clear();
    if token.chars().count() >= 2 {
        out.push(token);
    }
}

fn push_cjk_ngrams_vec(out: &mut Vec<String>, current: &mut String, cfg: &SearchConfig) {
    if current.is_empty() {
        return;
    }
    let chars = current.chars().collect::<Vec<_>>();
    current.clear();
    let n = normalized_ngram(cfg);
    if chars.len() < n {
        out.push(chars.into_iter().collect::<String>());
        return;
    }
    for window in chars.windows(n) {
        out.push(window.iter().collect::<String>());
    }
}

fn normalized_ngram(cfg: &SearchConfig) -> usize {
    cfg.ngram.clamp(1, 8)
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x309f // Hiragana
        | 0x30a0..=0x30ff // Katakana
        | 0x3400..=0x4dbf // CJK Extension A
        | 0x4e00..=0x9fff // CJK Unified Ideographs
        | 0xf900..=0xfaff // CJK Compatibility Ideographs
        | 0xff66..=0xff9f // Halfwidth Katakana
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

fn make_excerpt(text: &str, max_chars: usize) -> String {
    truncate_chars(text, max_chars).trim().to_string()
}

/// Extracts approximate searchable prose, skipping raw blocks and code spans.
/// This is not Typst evaluation: content generated by functions may be absent.
fn plain_text_for_search(body: &str) -> String {
    let mut out = String::new();
    let mut in_raw_block = false;
    let mut in_code = false;
    let mut skip_command = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_raw_block = !in_raw_block;
            continue;
        }
        if in_raw_block {
            continue;
        }
        for ch in line.chars() {
            if ch == '`' {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            if ch == '#' {
                skip_command = true;
                continue;
            }
            if skip_command {
                if ch.is_whitespace()
                    || ch == '['
                    || ch == ']'
                    || ch == '{'
                    || ch == '}'
                    || ch == ')'
                {
                    skip_command = false;
                }
                continue;
            }
            match ch {
                '=' | '*' | '_' | '[' | ']' | '{' | '}' | '(' | ')' | ',' => out.push(' '),
                _ => out.push(ch),
            }
        }
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::FrontMatter;
    use std::path::PathBuf;
    #[test]
    fn search_plain_text_ignores_code() {
        let got = plain_text_for_search("== Title\n`code` text #strong[ok]");
        assert!(got.contains("Title"));
        assert!(got.contains("text"));
        assert!(!got.contains("code"));
    }

    #[test]
    fn search_tokens_are_normalized_and_deduped() {
        let mut page = Page {
            source: PathBuf::from("content/posts/hello.typ"),
            rel: PathBuf::from("posts/hello.typ"),
            body: String::new(),
            processed_body: String::new(),
            meta: FrontMatter::default(),
            title: "Hello Rust".to_string(),
            url: "/posts/hello/".to_string(),
            canonical_url: "/posts/hello/".to_string(),
            slug: "hello".to_string(),
            path: "posts/hello".to_string(),
            file_path: "posts/hello.typ".to_string(),
            output_html: PathBuf::new(),
            output_pdf: PathBuf::new(),
            template: "base.typ".to_string(),
            section: "posts".to_string(),
            parent_section: None,
            ancestors: Vec::new(),
            toc: Vec::new(),
            hash: String::new(),
            prev_title: None,
            prev_url: None,
            next_title: None,
            next_url: None,
        };
        page.meta.tags = vec!["Typst".to_string(), "Rust".to_string()];
        let tokens = search_tokens(&page, "Rust rust HTML", "Typage");
        assert!(tokens.contains(&"rust".to_string()));
        assert!(tokens.contains(&"typst".to_string()));
        assert_eq!(tokens.iter().filter(|t| *t == "rust").count(), 1);
    }

    #[test]
    fn search_tokenizer_emits_cjk_ngrams() {
        let cfg = SearchConfig {
            mode: "auto".to_string(),
            ngram: 2,
            ..SearchConfig::default()
        };
        let tokens = tokenize_search_text("\u{65e5}\u{672c}\u{8a9e}\u{691c}\u{7d22} Typst", &cfg);
        assert!(tokens.contains(&"\u{65e5}\u{672c}".to_string()));
        assert!(tokens.contains(&"\u{672c}\u{8a9e}".to_string()));
        assert!(tokens.contains(&"\u{8a9e}\u{691c}".to_string()));
        assert!(tokens.contains(&"\u{691c}\u{7d22}".to_string()));
        assert!(tokens.contains(&"typst".to_string()));
    }

    #[test]
    fn search_config_accepts_bool_and_table() {
        let bool_cfg: Config = toml::from_str("search = false").unwrap();
        assert!(!bool_cfg.search.enabled);
        let table_cfg: Config = toml::from_str(
            "[search]\nenabled = true\nmode = \"ngram\"\nngram = 3\ncompact = true\n",
        )
        .unwrap();
        assert!(table_cfg.search.enabled);
        assert_eq!(table_cfg.search.mode, "ngram");
        assert_eq!(table_cfg.search.ngram, 3);
        assert!(table_cfg.search.compact);
        let cjk_cfg: Config =
            toml::from_str("[search]\nenabled = true\nmode = \"cjk\"\nngram = 2\n").unwrap();
        assert_eq!(cjk_cfg.search.mode, "cjk");
    }
}
