//! Section and taxonomy list construction, ordering, and pagination.
use super::outputs::output_path_for_url;
use super::section_url;
use crate::config::Config;
use crate::model::{FrontMatter, GeneratedPage, PageSummary};
use crate::util::slugify;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Builds virtual section and taxonomy listings from published page summaries.
/// Explicit content at a section's URL suppresses that generated section list.
pub(super) fn make_generated_pages(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
    section_meta: &BTreeMap<String, FrontMatter>,
) -> Vec<GeneratedPage> {
    let mut generated = Vec::new();
    let page_urls = pages
        .iter()
        .map(|page| page.url.as_str())
        .collect::<BTreeSet<_>>();
    let mut sections: BTreeMap<String, Vec<PageSummary>> = BTreeMap::new();
    for page in pages {
        sections
            .entry(page.section.clone())
            .or_default()
            .push(page.clone());
    }
    for (section, mut items) in sections {
        if section == "pages" {
            continue;
        }
        let meta = section_meta.get(&section);
        let sort_by = meta
            .and_then(|m| m.sort_by.as_deref())
            .unwrap_or("date_desc");
        sort_page_summaries(&mut items, sort_by);
        let title = meta
            .and_then(|m| m.title.clone())
            .unwrap_or_else(|| section.clone());
        let description = meta
            .and_then(|m| m.description.clone())
            .or_else(|| Some(format!("Pages in {section}")));
        let paginate_by = meta.and_then(|m| m.paginate_by).or(cfg.paginate_by);
        let url = section_url(&section);
        if page_urls.contains(url.as_str()) {
            continue;
        }
        push_paginated(
            &mut generated,
            out_root,
            ListPage {
                kind: "section",
                title,
                description,
                base_url: url,
            },
            items,
            paginate_by,
        );
    }

    for taxonomy in &cfg.taxonomies {
        let mut terms: BTreeMap<String, Vec<PageSummary>> = BTreeMap::new();
        for page in pages {
            for value in taxonomy_values(page, &taxonomy.name) {
                terms.entry(value).or_default().push(page.clone());
            }
        }
        let mut all_terms = Vec::new();
        for (term, mut items) in terms {
            sort_page_summaries(&mut items, "date_desc");
            let term_url = format!("/{}/{}/", taxonomy.slug, slugify(&term));
            all_terms.push(PageSummary {
                kind: "term".to_string(),
                title: term.clone(),
                url: term_url.clone(),
                canonical_url: term_url.clone(),
                slug: slugify(&term),
                path: term_url.trim_matches('/').to_string(),
                file_path: String::new(),
                description: Some(format!("{} page(s)", items.len())),
                date: None,
                updated: None,
                weight: None,
                section: taxonomy.name.clone(),
                parent_section: None,
                ancestors: Vec::new(),
                tags: Vec::new(),
                categories: Vec::new(),
                aliases: Vec::new(),
                source: String::new(),
                excerpt: None,
                toc: Vec::new(),
                fields: BTreeMap::new(),
            });
            push_paginated(
                &mut generated,
                out_root,
                ListPage {
                    kind: &taxonomy.name,
                    title: format!("{}: {}", taxonomy.name, term),
                    description: None,
                    base_url: term_url,
                },
                items,
                cfg.paginate_by,
            );
        }
        all_terms.sort_by(|a, b| a.title.cmp(&b.title));
        push_paginated(
            &mut generated,
            out_root,
            ListPage {
                kind: &format!("{}-index", taxonomy.name),
                title: taxonomy.name.clone(),
                description: Some(format!("All {}", taxonomy.name)),
                base_url: format!("/{}/", taxonomy.slug),
            },
            all_terms,
            cfg.paginate_by,
        );
    }
    generated
}

// URL tie-breakers keep navigation and pagination stable when primary keys match.
fn sort_page_summaries(items: &mut [PageSummary], sort_by: &str) {
    match sort_by {
        "weight" | "weight_asc" => items.sort_by(|a, b| {
            a.weight
                .unwrap_or(0)
                .cmp(&b.weight.unwrap_or(0))
                .then(a.url.cmp(&b.url))
        }),
        "weight_desc" => items.sort_by(|a, b| {
            b.weight
                .unwrap_or(0)
                .cmp(&a.weight.unwrap_or(0))
                .then(a.url.cmp(&b.url))
        }),
        "date" | "date_desc" => items.sort_by(|a, b| {
            b.date
                .cmp(&a.date)
                .then(a.weight.unwrap_or(0).cmp(&b.weight.unwrap_or(0)))
                .then(a.url.cmp(&b.url))
        }),
        "date_asc" => items.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then(a.weight.unwrap_or(0).cmp(&b.weight.unwrap_or(0)))
                .then(a.url.cmp(&b.url))
        }),
        "updated" | "updated_desc" => items.sort_by(|a, b| {
            b.updated
                .cmp(&a.updated)
                .then(b.date.cmp(&a.date))
                .then(a.url.cmp(&b.url))
        }),
        "updated_asc" => items.sort_by(|a, b| {
            a.updated
                .cmp(&b.updated)
                .then(a.date.cmp(&b.date))
                .then(a.url.cmp(&b.url))
        }),
        "title" | "title_asc" => {
            items.sort_by(|a, b| a.title.cmp(&b.title).then(a.url.cmp(&b.url)))
        }
        "title_desc" => items.sort_by(|a, b| b.title.cmp(&a.title).then(a.url.cmp(&b.url))),
        "url" | "url_asc" => items.sort_by(|a, b| a.url.cmp(&b.url)),
        "url_desc" => items.sort_by(|a, b| b.url.cmp(&a.url)),
        _ => items.sort_by(|a, b| {
            b.date
                .cmp(&a.date)
                .then(a.weight.unwrap_or(0).cmp(&b.weight.unwrap_or(0)))
                .then(a.url.cmp(&b.url))
        }),
    }
}

struct ListPage<'a> {
    kind: &'a str,
    title: String,
    description: Option<String>,
    base_url: String,
}

fn push_paginated(
    generated: &mut Vec<GeneratedPage>,
    out_root: &Path,
    list: ListPage<'_>,
    items: Vec<PageSummary>,
    paginate_by: Option<usize>,
) {
    let ListPage {
        kind,
        title,
        description,
        base_url,
    } = list;
    // An empty listing still needs one page; zero or absent limits disable
    // pagination. Keep the divisor nonzero in both cases.
    let page_size = paginate_by.filter(|n| *n > 0).unwrap_or(items.len().max(1));
    let total_pages = items.len().max(1).div_ceil(page_size);
    for idx in 0..total_pages {
        let page_number = idx + 1;
        let start = idx * page_size;
        let end = usize::min(start + page_size, items.len());
        let page_items = if start < items.len() {
            items[start..end].to_vec()
        } else {
            Vec::new()
        };
        let url = paginated_url(&base_url, page_number);
        let prev_url = if page_number > 1 {
            Some(paginated_url(&base_url, page_number - 1))
        } else {
            None
        };
        let next_url = if page_number < total_pages {
            Some(paginated_url(&base_url, page_number + 1))
        } else {
            None
        };
        generated.push(GeneratedPage {
            kind: kind.to_string(),
            title: if page_number == 1 {
                title.clone()
            } else {
                format!("{} - page {}", title, page_number)
            },
            description: description.clone(),
            output_html: output_path_for_url(out_root, &url)
                .unwrap_or_else(|_| out_root.join("invalid").join("index.html")),
            url,
            items: page_items,
            page_number,
            total_pages,
            prev_title: prev_url.as_ref().map(|_| "Previous".to_string()),
            prev_url,
            next_title: next_url.as_ref().map(|_| "Next".to_string()),
            next_url,
        });
    }
}

fn paginated_url(base_url: &str, page_number: usize) -> String {
    if page_number <= 1 {
        base_url.to_string()
    } else {
        format!("{}/page/{}/", base_url.trim_end_matches('/'), page_number)
    }
}

fn taxonomy_values(page: &PageSummary, name: &str) -> Vec<String> {
    match name {
        "tags" => return page.tags.clone(),
        "categories" => return page.categories.clone(),
        _ => {}
    }
    match page.fields.get(name).and_then(|field| field.export_toml()) {
        Some(toml::Value::String(s)) => vec![s.clone()],
        Some(toml::Value::Array(xs)) => xs
            .iter()
            .filter_map(|x| x.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paginated_urls_are_directory_urls() {
        assert_eq!(paginated_url("/posts/", 1), "/posts/");
        assert_eq!(paginated_url("/posts/", 2), "/posts/page/2/");
    }

    #[test]
    fn section_index_page_suppresses_generated_section_page() {
        let cfg = Config::default();
        let out = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("typage-section-index-test-{}", std::process::id()));
        let pages = vec![
            PageSummary {
                kind: "page".to_string(),
                title: "Awards".to_string(),
                url: "/awards/".to_string(),
                canonical_url: "/awards/".to_string(),
                slug: "awards".to_string(),
                path: "awards/index".to_string(),
                file_path: "awards/index.typ".to_string(),
                description: None,
                date: None,
                updated: None,
                weight: None,
                section: "awards".to_string(),
                parent_section: None,
                ancestors: Vec::new(),
                tags: Vec::new(),
                categories: Vec::new(),
                aliases: Vec::new(),
                source: "awards/index.typ".to_string(),
                excerpt: None,
                toc: Vec::new(),
                fields: BTreeMap::new(),
            },
            PageSummary {
                kind: "page".to_string(),
                title: "Child".to_string(),
                url: "/awards/child/".to_string(),
                canonical_url: "/awards/child/".to_string(),
                slug: "child".to_string(),
                path: "awards/child".to_string(),
                file_path: "awards/child.typ".to_string(),
                description: None,
                date: None,
                updated: None,
                weight: None,
                section: "awards".to_string(),
                parent_section: None,
                ancestors: Vec::new(),
                tags: Vec::new(),
                categories: Vec::new(),
                aliases: Vec::new(),
                source: "awards/child.typ".to_string(),
                excerpt: None,
                toc: Vec::new(),
                fields: BTreeMap::new(),
            },
        ];
        let generated = make_generated_pages(&cfg, &out, &pages, &BTreeMap::new());
        assert!(!generated.iter().any(|page| page.url == "/awards/"));
    }
}
