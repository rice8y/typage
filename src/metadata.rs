use crate::{
    compiler::{fingerprint, Compiler, Dependencies},
    model::{FrontMatter, MetadataField},
    util::typst_string,
};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use typst::foundations::{Dict, IntoValue, Module, Repr, Smart, Value};
use typst_syntax::ast::AstNode;
use typst_syntax::{ast, LinkedNode};

pub const CORE: &[&str] = &[
    "title",
    "description",
    "date",
    "updated",
    "expires",
    "weight",
    "lang",
    "draft",
    "slug",
    "path",
    "permalink",
    "template",
    "section",
    "tags",
    "categories",
    "aliases",
    "build_pdf",
    "excerpt",
    "toc",
    "sort_by",
    "paginate_by",
];

/// Evaluates collection schemas and keeps native values alive through rendering.
///
/// Rust consumes exportable routing fields; templates receive original Typst
/// values through the shared registry, including values JSON cannot represent.
pub struct MetadataContext {
    compiler: Arc<Compiler>,
    schemas: Dict,
    helpers: Dict,
    dependencies: Mutex<Dependencies>,
    values: Mutex<Dict>,
    sources: Mutex<BTreeMap<PathBuf, String>>,
    content_root: PathBuf,
}

impl std::fmt::Debug for MetadataContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataContext")
            .field("content_root", &self.content_root)
            .finish()
    }
}

fn exported(module: &Module, name: &str) -> Result<Value> {
    module
        .scope()
        .get(name)
        .map(|binding| binding.read().clone())
        .with_context(|| format!("missing `{name}` binding"))
}

impl MetadataContext {
    pub fn new(root: &Path, content_root: &Path, packages: PathBuf) -> Result<Self> {
        let compiler = Arc::new(Compiler::new(root, packages)?);
        let (module, _) = compiler.evaluate(
            &root.join(".typage-schema.typ"),
            include_str!("schema.typ"),
            &Dict::new(),
        )?;
        let helpers: Dict = module
            .scope()
            .iter()
            .map(|(name, binding)| (name.as_str().into(), binding.read().clone()))
            .collect();
        let path = content_root.join("config.typ");
        let mut dependencies = Dependencies::new();
        let schemas = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let source =
                format!("{raw}\n#let __typage_result = __resolve-collections(collections)\n");
            let (module, deps) = compiler.evaluate(&path, &source, &helpers)?;
            dependencies.extend(deps);
            let Value::Dict(schemas) = exported(&module, "__typage_result")? else {
                bail!("{}: collections must be a dictionary", path.display())
            };
            for (_, schema) in schemas.iter() {
                if !matches!(schema, Value::Dict(_)) {
                    bail!("{}: collection schema must be a dictionary", path.display());
                }
            }
            schemas
        } else {
            Dict::new()
        };
        Ok(Self {
            compiler,
            schemas,
            helpers,
            dependencies: Mutex::new(dependencies),
            values: Mutex::new(Dict::new()),
            sources: Mutex::new(BTreeMap::new()),
            content_root: content_root.to_path_buf(),
        })
    }

    pub fn values(&self) -> Dict {
        self.values.lock().expect("metadata poisoned").clone()
    }
    pub fn dependencies(&self) -> Dependencies {
        self.dependencies
            .lock()
            .expect("dependencies poisoned")
            .clone()
    }
    /// Fingerprints metadata inputs for the shared site graph, excluding bodies.
    /// Imported metadata dependencies still invalidate all graph consumers.
    pub fn hash(&self) -> String {
        let values = self
            .values()
            .iter()
            .map(|(key, value)| (key.to_string(), value.repr().to_string()))
            .collect::<BTreeMap<_, _>>();
        fingerprint(
            serde_json::to_string(&(
                self.dependencies(),
                &*self.sources.lock().expect("sources poisoned"),
                values,
            ))
            .unwrap()
            .as_bytes(),
        )
    }

    fn evaluate(&self, path: &Path, source: &str, bindings: &Dict) -> Result<Value> {
        let (module, mut deps) = self.compiler.evaluate(path, source, bindings)?;
        // The page's body is hashed by its compile job; only its metadata
        // expression belongs in the shared site graph.
        deps.remove(&crate::util::normalize_path(path)?);
        self.dependencies
            .lock()
            .expect("dependencies poisoned")
            .extend(deps);
        exported(&module, "__typage_result")
    }

    /// Evaluates one metadata source and returns the body without its directive.
    ///
    /// Schema validation and defaults happen before routing-field conversion.
    /// TOML/directive mixtures and multiple directives are rejected as ambiguous.
    pub fn parse(&self, path: &Path, raw: &str, section: &str) -> Result<(FrontMatter, String)> {
        let normalized = raw.replace("\r\n", "\n");
        let (frontmatter, body) = if let Some(rest) = normalized.strip_prefix("---\n") {
            let end = rest
                .find("\n---")
                .with_context(|| format!("{}:1:1: unclosed TOML front matter", path.display()))?;
            let value: toml::Value = toml::from_str(&rest[..end])?;
            (Some(from_toml(&value)), &normalized[4 + end + 4..])
        } else {
            (None, normalized.as_str())
        };
        let tree = typst_syntax::parse(body);
        let linked = LinkedNode::new(&tree);
        let mut directives = Vec::new();
        for node in linked.children() {
            let Some(show) = node.get().cast::<ast::ShowRule>() else {
                continue;
            };
            if show.selector().is_some() {
                continue;
            }
            let ast::Expr::FuncCall(call) = show.transform() else {
                continue;
            };
            let ast::Expr::FieldAccess(access) = call.callee() else {
                continue;
            };
            if access.field().as_str() != "with"
                || !matches!(access.target(), ast::Expr::Ident(name) if ["page", "project"].contains(&name.as_str()))
            {
                continue;
            }
            directives.push((
                node.range(),
                call.args().to_untyped().full_text().to_string(),
            ));
        }
        if frontmatter.is_some() && !directives.is_empty() {
            bail!(
                "{}: cannot combine TOML front matter with Typst metadata directive",
                path.display()
            );
        }
        if directives.len() > 1 {
            bail!("{}: multiple Typst metadata directives", path.display());
        }
        let (mut candidate, output_body) = if let Some((mut range, args)) = directives.pop() {
            if range.start > 0 && body.as_bytes()[range.start - 1] == b'#' {
                range.start -= 1;
            }
            // Evaluate named arguments as a dictionary, not as a rendering call.
            // Authors therefore need no `page` or `project` function in scope.
            let args = if args.trim() == "()" { "(:)" } else { &args };
            let prefix = metadata_prefix(body, range.start);
            let source = format!("{prefix}#let __typage_result = {args}\n");
            self.sources
                .lock()
                .expect("sources poisoned")
                .insert(path.to_path_buf(), fingerprint(source.as_bytes()));
            let value = self.evaluate(path, &source, &Dict::new())?;
            let mut output = body.to_string();
            while body[range.end..].starts_with('\n') {
                range.end += 1;
            }
            output.replace_range(range, "");
            (value, output)
        } else {
            self.sources.lock().expect("sources poisoned").insert(
                path.to_path_buf(),
                fingerprint(&normalized.as_bytes()[..normalized.len() - body.len()]),
            );
            (
                frontmatter.unwrap_or_else(|| Value::Dict(Dict::new())),
                body.trim_start_matches('\n').to_string(),
            )
        };
        let Value::Dict(ref dict) = candidate else {
            bail!("{}: metadata must contain named arguments", path.display())
        };
        if dict.contains("extra") {
            bail!(
                "{}: metadata field `extra` is not supported; declare fields directly",
                path.display()
            );
        }
        let section = dict
            .get("section")
            .ok()
            .and_then(|v| {
                if let Value::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or(section);
        if let Ok(schema) = self.schemas.get(section).or_else(|_| {
            self.schemas
                .get(section.split('/').next().unwrap_or(section))
        }) {
            if path.file_name() != Some(std::ffi::OsStr::new("_index.typ")) {
                let mut bindings = self.helpers.clone();
                bindings.insert("__candidate".into(), candidate.clone());
                bindings.insert("__schema".into(), schema.clone());
                let core = CORE
                    .iter()
                    .map(|name| typst_string(name))
                    .collect::<Vec<_>>()
                    .join(", ");
                candidate = self.evaluate(path, &format!("#let __typage_result = __validate(__candidate, __schema, source: {}, core: ({core}))", typst_string(&path.display().to_string())), &bindings)?;
            }
        }
        let Value::Dict(dict) = candidate else {
            unreachable!()
        };
        let mut core = serde_json::Map::new();
        let mut fields = BTreeMap::new();
        for (name, value) in dict.iter() {
            let name = name.as_str();
            if CORE.contains(&name) {
                let exported = export_json(value).with_context(|| {
                    format!("{}: core field `{name}` must be exportable", path.display())
                })?;
                core.insert(
                    if name == "path" { "permalink" } else { name }.to_string(),
                    exported,
                );
                if !matches!(value, Value::Datetime(_)) {
                    continue;
                }
            }
            // Qualify registry keys by source so equal field names on different
            // pages cannot overwrite each other during current-value hydration.
            let key = fingerprint(format!("{}:{name}", path.display()).as_bytes());
            self.values
                .lock()
                .expect("metadata poisoned")
                .insert(key.as_str().into(), value.clone());
            fields.insert(name.to_string(), MetadataField::native(key, value.clone()));
        }
        let mut meta: FrontMatter = serde_json::from_value(serde_json::Value::Object(core))
            .with_context(|| format!("{}: invalid core metadata", path.display()))?;
        meta.fields = fields;
        if meta.date.is_none() {
            meta.date = meta
                .fields
                .get("publishedDate")
                .and_then(|f| f.export_json())
                .and_then(|v| v.as_str().map(str::to_string));
        }
        if meta.updated.is_none() {
            meta.updated = meta
                .fields
                .get("updatedDate")
                .and_then(|f| f.export_json())
                .and_then(|v| v.as_str().map(str::to_string));
        }
        Ok((meta, output_body))
    }
}

fn metadata_prefix(body: &str, end: usize) -> String {
    let prefix = &body[..end];
    let tree = typst_syntax::parse(prefix);
    let mut bytes = prefix.as_bytes().to_vec();
    for node in LinkedNode::new(&tree).children() {
        let Some(import) = node.get().cast::<ast::ModuleImport>() else {
            continue;
        };
        let ast::Expr::Str(source) = import.source() else {
            continue;
        };
        let source = source.get();
        if !source.starts_with("@local/typage:") && !source.starts_with("@local/typage-theme:") {
            continue;
        }
        // Generated packages exist only after discovery. Keep these imports in
        // the page body, but do not evaluate rendering-only imports as metadata.
        let mut range = node.range();
        if range.start > 0 && bytes[range.start - 1] == b'#' {
            range.start -= 1;
        }
        for byte in &mut bytes[range] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).expect("metadata prefix remains UTF-8")
}

pub fn from_toml(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(v) => v.as_str().into_value(),
        toml::Value::Integer(v) => (*v).into_value(),
        toml::Value::Float(v) => (*v).into_value(),
        toml::Value::Boolean(v) => (*v).into_value(),
        toml::Value::Datetime(v) => v.to_string().into_value(),
        toml::Value::Array(v) => Value::Array(v.iter().map(from_toml).collect()),
        toml::Value::Table(v) => Value::Dict(
            v.iter()
                .map(|(name, value)| (name.as_str().into(), from_toml(value)))
                .collect(),
        ),
    }
}

pub fn export_json(value: &Value) -> Option<serde_json::Value> {
    use serde_json::Value as Json;
    Some(match value {
        Value::None => Json::Null,
        Value::Bool(v) => Json::Bool(*v),
        Value::Int(v) => Json::from(*v),
        Value::Float(v) => Json::Number(serde_json::Number::from_f64(*v)?),
        Value::Str(v) => Json::String(v.to_string()),
        Value::Datetime(v) => Json::String(v.display(Smart::Auto).ok()?.to_string()),
        Value::Array(v) => Json::Array(v.iter().map(export_json).collect::<Option<_>>()?),
        Value::Dict(v) => Json::Object(
            v.iter()
                .map(|(k, v)| Some((k.to_string(), export_json(v)?)))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}
