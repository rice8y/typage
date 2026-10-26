//! Sitemap, robots.txt, and RSS/Atom serialization.
use super::outputs::{public_file_path, public_file_url};
use super::{absolute_url, escape_xml};
use crate::config::{Config, FeedConfig};
use crate::model::{GeneratedPage, PageSummary};
use crate::util::write_if_changed;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) fn write_sitemap(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
    generated: &[GeneratedPage],
) -> Result<()> {
    if cfg.base_url.trim().is_empty() {
        eprintln!("warning: sitemap.xml skipped because base_url is empty");
        return Ok(());
    }
    let mut urls: BTreeMap<String, Option<String>> = BTreeMap::new();
    for page in pages {
        let entry = urls.entry(page.url.clone()).or_insert(None);
        *entry = newest_date(
            entry.clone(),
            page.updated.clone().or_else(|| page.date.clone()),
        );
    }
    for gen in generated {
        let latest = latest_page_date(&gen.items);
        let entry = urls.entry(gen.url.clone()).or_insert(None);
        *entry = newest_date(entry.clone(), latest);
    }
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for (url, lastmod) in urls {
        xml.push_str("  <url>\n    <loc>");
        xml.push_str(&escape_xml(&absolute_url(cfg, &url)));
        xml.push_str("</loc>\n");
        if let Some(lastmod) = lastmod {
            xml.push_str("    <lastmod>");
            xml.push_str(&escape_xml(&lastmod));
            xml.push_str("</lastmod>\n");
        }
        xml.push_str("  </url>\n");
    }
    xml.push_str("</urlset>\n");
    write_if_changed(&out_root.join("sitemap.xml"), &xml)
}

/// Writes the primary RSS feed plus configured section feeds from one page graph.
/// An empty base URL deliberately leaves site links relative for local previews.
pub(super) fn write_configured_feeds(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
) -> Result<()> {
    let primary = FeedConfig {
        path: cfg.feed_path.clone(),
        title: cfg.feed_title.clone(),
        description: cfg.feed_description.clone(),
        link: cfg.feed_link.clone(),
        section: None,
        sections: cfg.feed_sections.clone(),
        limit: cfg.feed_limit,
    };
    write_rss_feed(cfg, out_root, pages, &primary)?;
    for feed in &cfg.feeds {
        write_rss_feed(cfg, out_root, pages, feed)?;
    }
    Ok(())
}

fn feed_items(cfg: &Config, pages: &[PageSummary], spec: &FeedConfig) -> Vec<PageSummary> {
    let mut sections = BTreeSet::new();
    if let Some(section) = &spec.section {
        if !section.trim().is_empty() {
            sections.insert(section.clone());
        }
    }
    for section in &spec.sections {
        if !section.trim().is_empty() {
            sections.insert(section.clone());
        }
    }
    if sections.is_empty() {
        for section in &cfg.feed_sections {
            if !section.trim().is_empty() {
                sections.insert(section.clone());
            }
        }
    }

    let mut items = pages
        .iter()
        .filter(|page| sections.is_empty() || sections.contains(&page.section))
        .cloned()
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.date.cmp(&a.date).then(a.url.cmp(&b.url)));
    if spec.limit > 0 {
        items.truncate(spec.limit);
    }
    items
}

fn feed_description<'a>(cfg: &'a Config, spec: &'a FeedConfig) -> &'a str {
    spec.description
        .as_deref()
        .or(cfg.feed_description.as_deref())
        .or_else(|| cfg.extra.get("description").and_then(|v| v.as_str()))
        .unwrap_or(&cfg.title)
}

fn feed_title<'a>(cfg: &'a Config, spec: &'a FeedConfig) -> &'a str {
    spec.title
        .as_deref()
        .or(cfg.feed_title.as_deref())
        .unwrap_or(&cfg.title)
}

fn write_rss_feed(
    cfg: &Config,
    out_root: &Path,
    pages: &[PageSummary],
    spec: &FeedConfig,
) -> Result<()> {
    let items = feed_items(cfg, pages, spec);
    let link = if spec.link.trim().is_empty() {
        "/"
    } else {
        spec.link.trim()
    };
    let self_url = public_file_url(&spec.path)?;
    let latest = latest_page_date(&items).map(|d| rss_date(&d));
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rss version=\"2.0\"><channel><title>{}</title><link>{}</link><description>{}</description>\n",
        escape_xml(feed_title(cfg, spec)),
        escape_xml(&absolute_url(cfg, link)),
        escape_xml(feed_description(cfg, spec))
    );
    xml.push_str(&format!("<atom:link xmlns:atom=\"http://www.w3.org/2005/Atom\" href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>\n", escape_xml(&absolute_url(cfg, &self_url))));
    if let Some(date) = latest {
        xml.push_str(&format!(
            "<lastBuildDate>{}</lastBuildDate>\n",
            escape_xml(&date)
        ));
    }
    for page in items {
        xml.push_str("<item>");
        xml.push_str(&format!("<title>{}</title>", escape_xml(&page.title)));
        xml.push_str(&format!(
            "<link>{}</link>",
            escape_xml(&absolute_url(cfg, &page.url))
        ));
        xml.push_str(&format!(
            "<guid isPermaLink=\"true\">{}</guid>",
            escape_xml(&absolute_url(cfg, &page.url))
        ));
        if let Some(date) = &page.date {
            xml.push_str(&format!(
                "<pubDate>{}</pubDate>",
                escape_xml(&rss_date(date))
            ));
        }
        let desc = page.excerpt.as_ref().or(page.description.as_ref());
        if let Some(desc) = desc {
            xml.push_str(&format!("<description>{}</description>", escape_xml(desc)));
        }
        xml.push_str("</item>\n");
    }
    xml.push_str("</channel></rss>\n");
    write_if_changed(&public_file_path(out_root, &spec.path)?, &xml)
}

pub(super) fn write_atom_feed(cfg: &Config, out_root: &Path, pages: &[PageSummary]) -> Result<()> {
    let spec = FeedConfig {
        path: cfg.atom_path.clone(),
        title: cfg.feed_title.clone(),
        description: cfg.feed_description.clone(),
        link: cfg.feed_link.clone(),
        section: None,
        sections: cfg.feed_sections.clone(),
        limit: cfg.feed_limit,
    };
    let items = feed_items(cfg, pages, &spec);
    let self_url = public_file_url(&cfg.atom_path)?;
    let updated = latest_page_date(&items)
        .map(|d| atom_date(&d))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n  <title>{}</title>\n  <id>{}</id>\n  <link href=\"{}\"/>\n  <link rel=\"self\" href=\"{}\"/>\n  <updated>{}</updated>\n",
        escape_xml(feed_title(cfg, &spec)),
        escape_xml(&absolute_url(cfg, "/")),
        escape_xml(&absolute_url(cfg, "/")),
        escape_xml(&absolute_url(cfg, &self_url)),
        escape_xml(&updated),
    );
    for page in items {
        let url = absolute_url(cfg, &page.url);
        let updated = page
            .updated
            .as_ref()
            .or(page.date.as_ref())
            .map(|d| atom_date(d))
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
        xml.push_str("  <entry>\n");
        xml.push_str(&format!("    <title>{}</title>\n", escape_xml(&page.title)));
        xml.push_str(&format!("    <id>{}</id>\n", escape_xml(&url)));
        xml.push_str(&format!("    <link href=\"{}\"/>\n", escape_xml(&url)));
        xml.push_str(&format!(
            "    <updated>{}</updated>\n",
            escape_xml(&updated)
        ));
        let summary = page.excerpt.as_ref().or(page.description.as_ref());
        if let Some(summary) = summary {
            xml.push_str(&format!("    <summary>{}</summary>\n", escape_xml(summary)));
        }
        xml.push_str("  </entry>\n");
    }
    xml.push_str("</feed>\n");
    write_if_changed(&public_file_path(out_root, &cfg.atom_path)?, &xml)
}

pub(super) fn write_robots(cfg: &Config, out_root: &Path) -> Result<()> {
    let mut text = String::from("User-agent: *\nAllow: /\n");
    if cfg.sitemap && !cfg.base_url.trim().is_empty() {
        text.push('\n');
        text.push_str("Sitemap: ");
        text.push_str(&absolute_url(cfg, "/sitemap.xml"));
        text.push('\n');
    }
    write_if_changed(&out_root.join("robots.txt"), &text)
}

fn latest_page_date(items: &[PageSummary]) -> Option<String> {
    items
        .iter()
        .filter_map(|page| page.updated.clone().or_else(|| page.date.clone()))
        .max()
}

fn newest_date(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn atom_date(date: &str) -> String {
    if date.split('-').count() == 3 {
        format!("{date}T00:00:00Z")
    } else {
        date.to_string()
    }
}

fn rss_date(date: &str) -> String {
    let parts = date.split('-').collect::<Vec<_>>();
    if parts.len() == 3 {
        if let Ok(month) = parts[1].parse::<usize>() {
            let month_name = [
                "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
                "Dec",
            ]
            .get(month)
            .copied()
            .unwrap_or("");
            if !month_name.is_empty() {
                return format!("{} {} {} 00:00:00 +0000", parts[2], month_name, parts[0]);
            }
        }
    }
    date.to_string()
}

#[cfg(test)]
mod tests {
    use super::super::tests::page_summary;
    use super::*;
    #[test]
    fn rss_date_formats_simple_dates() {
        assert_eq!(rss_date("2026-06-23"), "23 Jun 2026 00:00:00 +0000");
        assert_eq!(atom_date("2026-06-23"), "2026-06-23T00:00:00Z");
    }

    #[test]
    fn feeds_are_written_without_base_url() {
        let tmp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("typage-feed-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = Config {
            base_url: String::new(),
            ..Config::default()
        };
        let pages = vec![page_summary("Hello", "/hello/", "posts", "2026-06-23")];

        write_configured_feeds(&cfg, &tmp, &pages).unwrap();
        write_atom_feed(&cfg, &tmp, &pages).unwrap();

        let rss = std::fs::read_to_string(tmp.join("feed.xml")).unwrap();
        let atom = std::fs::read_to_string(tmp.join("atom.xml")).unwrap();
        assert!(rss.contains("<link>/</link>"));
        assert!(rss.contains("<link>/hello/</link>"));
        assert!(atom.contains("<link href=\"/\"/>"));
        assert!(atom.contains("<link href=\"/hello/\"/>"));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn configured_rss_feeds_filter_sections_and_paths() {
        let tmp = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "typage-configured-feed-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = Config {
            base_url: String::new(),
            feed_path: "rss.xml".to_string(),
            feed_sections: vec!["blog".to_string()],
            feed_limit: 0,
            feeds: vec![FeedConfig {
                path: "projects/rss.xml".to_string(),
                title: Some("Projects".to_string()),
                description: Some("Project feed".to_string()),
                link: "/projects/".to_string(),
                section: Some("projects".to_string()),
                limit: 0,
                ..FeedConfig::default()
            }],
            ..Config::default()
        };
        let pages = vec![
            page_summary("Blog", "/blog/post/", "blog", "2026-06-23"),
            page_summary("Project", "/projects/tool/", "projects", "2026-06-22"),
            page_summary("Favorite", "/favorites/art/", "favorites", "2026-06-21"),
        ];

        write_configured_feeds(&cfg, &tmp, &pages).unwrap();

        let rss = std::fs::read_to_string(tmp.join("rss.xml")).unwrap();
        let projects = std::fs::read_to_string(tmp.join("projects/rss.xml")).unwrap();
        assert!(rss.contains("<title>Blog</title>"));
        assert!(!rss.contains("<title>Project</title>"));
        assert!(!rss.contains("<title>Favorite</title>"));
        assert!(projects.contains("<title>Project</title>"));
        assert!(!projects.contains("<title>Blog</title>"));
        assert!(!projects.contains("<title>Favorite</title>"));
        let _ = std::fs::remove_dir_all(tmp);
    }
}
