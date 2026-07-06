use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub title: String,
    pub base_url: String,
    pub lang: String,
    pub typst: String,
    pub content_dir: PathBuf,
    pub templates_dir: PathBuf,
    pub static_dir: PathBuf,
    pub out_dir: PathBuf,
    /// Optional page permalink pattern, e.g. `/:section/:slug/`.
    pub permalink: Option<String>,
    pub theme: Option<String>,
    pub cache_dir: PathBuf,
    pub default_template: String,
    pub list_template: String,
    pub features: String,
    pub bundle_features: String,
    pub build_pdf: bool,
    /// Include pages with future `date` values.
    pub build_future: bool,
    /// Include pages whose `expires` date is before today.
    pub build_expired: bool,
    pub fail_on_broken_links: bool,
    pub paginate_by: Option<usize>,
    pub feed: bool,
    pub feed_path: String,
    pub feed_title: Option<String>,
    pub feed_description: Option<String>,
    pub feed_link: String,
    pub feed_sections: Vec<String>,
    /// Maximum RSS items. 0 means unlimited.
    pub feed_limit: usize,
    pub atom_feed: bool,
    pub atom_path: String,
    pub feeds: Vec<FeedConfig>,
    pub pdf_documents: Vec<PdfDocumentConfig>,
    pub sitemap: bool,
    #[serde(default, deserialize_with = "deserialize_search_config")]
    pub search: SearchConfig,
    /// Generate robots.txt.
    pub robots: bool,
    /// Number of parallel Typst compile jobs. 0 means auto.
    pub jobs: usize,
    pub extra: BTreeMap<String, toml::Value>,
    pub scripts: BTreeMap<String, String>,
    /// Command hooks such as pre_build/post_build.
    pub hooks: BTreeMap<String, String>,
    pub plugins: BTreeMap<String, PluginConfig>,
    pub taxonomies: Vec<TaxonomyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub enabled: bool,
    /// `auto`, `latin`, `cjk`, `ngram`, or `ngram-only`.
    pub mode: String,
    /// CJK n-gram width used when mode is `auto` or `ngram`.
    pub ngram: usize,
    pub include_body: bool,
    pub include_headings: bool,
    pub include_tags: bool,
    pub include_taxonomies: bool,
    /// When true, omit full body text from search_index.json and rely on tokens/fields.
    pub compact: bool,
    pub max_tokens: usize,
    pub max_body_chars: usize,
    pub max_heading_chars: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "auto".to_string(),
            ngram: 2,
            include_body: true,
            include_headings: true,
            include_tags: true,
            include_taxonomies: true,
            compact: false,
            max_tokens: 2048,
            max_body_chars: 20_000,
            max_heading_chars: 240,
        }
    }
}

fn deserialize_search_config<'de, D>(deserializer: D) -> std::result::Result<SearchConfig, D::Error>
where
    D: Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    match value {
        toml::Value::Boolean(enabled) => Ok(SearchConfig {
            enabled,
            ..SearchConfig::default()
        }),
        other => other.try_into().map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TaxonomyConfig {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedConfig {
    pub path: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub link: String,
    pub section: Option<String>,
    pub sections: Vec<String>,
    /// Maximum RSS items. 0 means unlimited.
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    /// Installation source such as `crates.io`, `git`, `path`, or `command`.
    pub source: String,
    /// Cargo package name, for example `typage-plugin-typst-docs`.
    pub package: Option<String>,
    pub version: Option<String>,
    pub git: Option<String>,
    pub path: Option<PathBuf>,
    /// Executable name or path. Defaults to `package` or `typage-plugin-<name>`.
    pub command: Option<String>,
    /// Extra arguments appended when Typage invokes the plugin.
    pub args: Vec<String>,
    /// Source paths watched by `typage watch` and `serve --live-reload`.
    pub watch: Vec<PathBuf>,
    /// Project paths generated or managed by this plugin.
    pub outputs: Vec<PathBuf>,
    /// Disable without removing the plugin entry.
    pub disabled: bool,
    /// Plugin-specific configuration passed through as JSON.
    pub config: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfDocumentConfig {
    /// Output PDF path relative to `out_dir`, for example `print.pdf`.
    pub path: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Template path relative to `templates_dir`.
    pub template: String,
    pub sections: Vec<String>,
    pub pages: Vec<String>,
    /// Sections that should receive their own heading in combined PDF output.
    pub section_headings: Vec<String>,
    /// Heading level used for generated section headings.
    pub section_heading_level: usize,
    pub sort_by: Option<String>,
    pub include_drafts: Option<bool>,
    pub include_future: Option<bool>,
    pub include_expired: Option<bool>,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            path: "feed.xml".to_string(),
            title: None,
            description: None,
            link: "/".to_string(),
            section: None,
            sections: Vec::new(),
            limit: 20,
        }
    }
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            source: "crates.io".to_string(),
            package: None,
            version: None,
            git: None,
            path: None,
            command: None,
            args: Vec::new(),
            watch: Vec::new(),
            outputs: Vec::new(),
            disabled: false,
            config: BTreeMap::new(),
        }
    }
}

impl Default for PdfDocumentConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            title: None,
            description: None,
            template: "print.typ".to_string(),
            sections: Vec::new(),
            pages: Vec::new(),
            section_headings: Vec::new(),
            section_heading_level: 1,
            sort_by: None,
            include_drafts: None,
            include_future: None,
            include_expired: None,
        }
    }
}

impl Default for TaxonomyConfig {
    fn default() -> Self {
        Self {
            name: "tags".to_string(),
            slug: "tags".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: "Typage".to_string(),
            base_url: String::new(),
            lang: "en".to_string(),
            typst: "typst".to_string(),
            content_dir: PathBuf::from("content"),
            templates_dir: PathBuf::from("templates"),
            static_dir: PathBuf::from("static"),
            out_dir: PathBuf::from("public"),
            permalink: None,
            theme: None,
            cache_dir: PathBuf::from(".typage"),
            default_template: "base.typ".to_string(),
            list_template: "list.typ".to_string(),
            features: "html".to_string(),
            bundle_features: "bundle,html".to_string(),
            build_pdf: false,
            build_future: false,
            build_expired: false,
            fail_on_broken_links: true,
            paginate_by: None,
            feed: true,
            feed_path: "feed.xml".to_string(),
            feed_title: None,
            feed_description: None,
            feed_link: "/".to_string(),
            feed_sections: Vec::new(),
            feed_limit: 20,
            atom_feed: true,
            atom_path: "atom.xml".to_string(),
            feeds: Vec::new(),
            pdf_documents: Vec::new(),
            sitemap: true,
            search: SearchConfig::default(),
            robots: true,
            jobs: 0,
            extra: BTreeMap::new(),
            scripts: BTreeMap::new(),
            hooks: BTreeMap::new(),
            plugins: BTreeMap::new(),
            taxonomies: vec![
                TaxonomyConfig {
                    name: "tags".to_string(),
                    slug: "tags".to_string(),
                },
                TaxonomyConfig {
                    name: "categories".to_string(),
                    slug: "categories".to_string(),
                },
            ],
        }
    }
}

pub fn load_config(root: &Path) -> Result<Config> {
    let path = root.join("config.toml");
    if !path.exists() {
        let mut cfg = Config::default();
        merge_plugin_config(root, &mut cfg)?;
        return Ok(cfg);
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut cfg =
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    merge_plugin_config(root, &mut cfg)?;
    Ok(cfg)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PluginConfigFile {
    plugins: BTreeMap<String, PluginConfig>,
}

fn merge_plugin_config(root: &Path, cfg: &mut Config) -> Result<()> {
    let path = root.join("typage.plugins.toml");
    if !path.exists() {
        return Ok(());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let file: PluginConfigFile =
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    for (name, plugin) in file.plugins {
        cfg.plugins.insert(name, plugin);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_configured_feeds() {
        let cfg: Config = toml::from_str(
            r#"
title = "Site"
feed = true
feed_path = "rss.xml"
feed_sections = ["blog", "projects"]
feed_limit = 0
atom_feed = false

[[feeds]]
path = "projects/rss.xml"
title = "Projects"
description = "Project updates."
link = "/projects/"
section = "projects"
limit = 0
"#,
        )
        .unwrap();

        assert_eq!(cfg.feed_path, "rss.xml");
        assert_eq!(cfg.feed_sections, vec!["blog", "projects"]);
        assert_eq!(cfg.feed_limit, 0);
        assert!(!cfg.atom_feed);
        assert_eq!(cfg.feeds.len(), 1);
        assert_eq!(cfg.feeds[0].path, "projects/rss.xml");
        assert_eq!(cfg.feeds[0].section.as_deref(), Some("projects"));
        assert_eq!(cfg.feeds[0].limit, 0);
    }

    #[test]
    fn parses_pdf_documents() {
        let cfg: Config = toml::from_str(
            r#"
[[pdf_documents]]
path = "print.pdf"
title = "Print"
description = "Combined print document."
template = "print.typ"
sections = ["posts", "projects"]
section_headings = ["posts"]
section_heading_level = 1
sort_by = "date_desc"
include_drafts = false

[[pdf_documents]]
path = "projects.pdf"
pages = ["projects/typage.typ", "/projects/typshade/"]
"#,
        )
        .unwrap();

        assert_eq!(cfg.pdf_documents.len(), 2);
        assert_eq!(cfg.pdf_documents[0].path, "print.pdf");
        assert_eq!(cfg.pdf_documents[0].template, "print.typ");
        assert_eq!(cfg.pdf_documents[0].sections, vec!["posts", "projects"]);
        assert_eq!(cfg.pdf_documents[0].section_headings, vec!["posts"]);
        assert_eq!(cfg.pdf_documents[0].section_heading_level, 1);
        assert_eq!(cfg.pdf_documents[0].include_drafts, Some(false));
        assert_eq!(
            cfg.pdf_documents[1].pages,
            vec!["projects/typage.typ", "/projects/typshade/"]
        );
    }

    #[test]
    fn parses_configured_plugins() {
        let cfg: Config = toml::from_str(
            r#"
[plugins.typst_docs]
package = "typage-plugin-typst-docs"
version = "0.1.0"
watch = ["packages/demo/**/*.typ"]
outputs = ["content/api"]

[plugins.typst_docs.config]
source_dir = "packages/demo"
out_dir = "content/api"
"#,
        )
        .unwrap();

        let plugin = cfg.plugins.get("typst_docs").unwrap();
        assert_eq!(plugin.source, "crates.io");
        assert_eq!(plugin.package.as_deref(), Some("typage-plugin-typst-docs"));
        assert_eq!(plugin.version.as_deref(), Some("0.1.0"));
        assert_eq!(plugin.watch, vec![PathBuf::from("packages/demo/**/*.typ")]);
        assert_eq!(plugin.outputs, vec![PathBuf::from("content/api")]);
        assert_eq!(
            plugin
                .config
                .get("source_dir")
                .and_then(toml::Value::as_str),
            Some("packages/demo")
        );
    }

    #[test]
    fn load_config_merges_plugin_file() {
        let tmp =
            std::env::temp_dir().join(format!("typage-plugin-config-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(
            tmp.join("config.toml"),
            r#"
title = "Site"

[plugins.inline]
command = "inline-plugin"
"#,
        )
        .unwrap();
        fs::write(
            tmp.join("typage.plugins.toml"),
            r#"
[plugins.docs]
source = "command"
command = "docs-plugin"

[plugins.docs.config]
source_dir = "src"
"#,
        )
        .unwrap();

        let cfg = load_config(&tmp).unwrap();
        assert_eq!(cfg.title, "Site");
        assert_eq!(
            cfg.plugins.get("inline").and_then(|p| p.command.as_deref()),
            Some("inline-plugin")
        );
        assert_eq!(
            cfg.plugins.get("docs").and_then(|p| p.command.as_deref()),
            Some("docs-plugin")
        );
        let _ = fs::remove_dir_all(tmp);
    }
}
