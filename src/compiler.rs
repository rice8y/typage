//! Embedded Typst compilation, with per-job sources and shared fonts/packages.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{bail, Context, Result};
use comemo::Track;
use typst::diag::{FileError, FileResult, SourceDiagnostic, SourceResult};
use typst::foundations::{Binding, Bytes, Datetime, Dict, Duration, IntoValue, Module};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, LibraryExt, World};
use typst_kit::datetime::Time;
use typst_kit::diagnostics::{self, termcolor::Buffer, DiagnosticFormat, DiagnosticWorld};
use typst_kit::downloader::SystemDownloader;
use typst_kit::files::{FileLoader, FileStore, FsRoot};
use typst_kit::fonts::{self, FontStore};
use typst_kit::packages::{FsPackages, SystemPackages, UniversePackages};

// Keep this aligned with the exact versions in Cargo.toml. It also invalidates
// output caches when the compiler changes independently of Typage's version.
pub const VERSION: &str = "0.15.1";

pub type Dependencies = BTreeMap<PathBuf, Option<String>>;

pub fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn changed_dependencies(dependencies: &Dependencies) -> Vec<PathBuf> {
    dependencies
        .iter()
        .filter_map(|(path, expected)| {
            let actual = fs::read(path).ok().map(|bytes| fingerprint(&bytes));
            (actual != *expected).then(|| path.clone())
        })
        .collect()
}

pub struct Compiler {
    root: PathBuf,
    local_packages: FsPackages,
    packages: Mutex<SystemPackages>,
    fonts: OnceLock<FontStore>,
    font_paths: Vec<PathBuf>,
    ignore_system_fonts: bool,
    ignore_embedded_fonts: bool,
    timestamp: Option<i64>,
    metadata: RwLock<Dict>,
    metadata_dependencies: RwLock<Dependencies>,
}

impl Compiler {
    pub fn new(root: &Path, package_path: PathBuf) -> Result<Self> {
        let root = root.canonicalize()?;
        let user_agent = concat!("typage/", env!("CARGO_PKG_VERSION"));
        let downloader = match std::env::var_os("TYPST_CERT") {
            Some(path) => SystemDownloader::with_cert_path(user_agent, root.join(path)),
            None => SystemDownloader::new(user_agent),
        };
        let packages = SystemPackages::from_parts(
            env_path("TYPST_PACKAGE_PATH", &root)
                .map(FsPackages::new)
                .or_else(FsPackages::system_data),
            env_path("TYPST_PACKAGE_CACHE_PATH", &root)
                .map(FsPackages::new)
                .or_else(FsPackages::system_cache),
            UniversePackages::new(downloader),
        );
        let font_paths = std::env::var_os("TYPST_FONT_PATHS")
            .map(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| root.join(p))
                    .collect()
            })
            .unwrap_or_default();
        let timestamp = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .map(|value| value.parse::<i64>().context("invalid SOURCE_DATE_EPOCH"))
            .transpose()?;
        if let Some(timestamp) = timestamp {
            Time::fixed_timestamp(timestamp).map_err(|err| anyhow::anyhow!(err))?;
        }
        Ok(Self {
            root,
            local_packages: FsPackages::new(package_path),
            packages: Mutex::new(packages),
            fonts: OnceLock::new(),
            font_paths,
            ignore_system_fonts: env_bool("TYPST_IGNORE_SYSTEM_FONTS")?,
            ignore_embedded_fonts: env_bool("TYPST_IGNORE_EMBEDDED_FONTS")?,
            timestamp,
            metadata: RwLock::new(Dict::new()),
            metadata_dependencies: RwLock::new(Dependencies::new()),
        })
    }

    fn fonts(&self) -> &FontStore {
        self.fonts.get_or_init(|| {
            let mut store = FontStore::new();
            for path in &self.font_paths {
                store.extend(fonts::scan(path));
            }
            if !self.ignore_system_fonts {
                store.extend(fonts::system());
            }
            if !self.ignore_embedded_fonts {
                store.extend(fonts::embedded());
            }
            store
        })
    }

    pub fn set_metadata(&self, values: Dict, dependencies: Dependencies) {
        *self.metadata.write().expect("metadata poisoned") = values;
        *self
            .metadata_dependencies
            .write()
            .expect("metadata dependencies poisoned") = dependencies;
    }

    pub fn evaluate(
        &self,
        path: &Path,
        source: &str,
        bindings: &Dict,
    ) -> Result<(Module, Dependencies)> {
        use typst::engine::{Route, Sink, Traced};
        let mut world = CompileWorld::new(self, path, "html", &[])?;
        let mut library = (*world.library).clone();
        for (name, value) in bindings.iter() {
            library
                .global
                .scope_mut()
                .bind(name.clone().into(), Binding::detached(value.clone()));
        }
        world.library = LazyHash::new(library);
        let source = Source::new(world.main, source.to_string());
        world.overlay = Some(source.clone());
        world
            .dependencies
            .lock()
            .expect("dependencies poisoned")
            .insert(
                crate::util::normalize_path(path)?,
                fs::read(path).ok().map(|bytes| fingerprint(&bytes)),
            );
        let mut sink = Sink::new();
        let module = world.result(typst_eval::eval(
            (&world as &dyn World).track(),
            &world.library,
            Traced::default().track(),
            sink.track_mut(),
            Route::default().track(),
            &source,
        ))?;
        let dependencies = world
            .dependencies
            .lock()
            .expect("dependencies poisoned")
            .clone();
        Ok((module, dependencies))
    }

    pub fn compile(
        &self,
        input: &Path,
        output: &Path,
        format: &str,
        features: &str,
        inputs: &[(&str, &str)],
    ) -> Result<Dependencies> {
        let world = CompileWorld::new(self, input, features, inputs)?;
        match format {
            "html" => {
                let doc = world.compile::<typst_html::HtmlDocument>()?;
                let html = world.result(typst_html::html(&doc, &Default::default()))?;
                write_output(output, html.as_bytes())?;
            }
            "pdf" => {
                let doc = world.compile::<typst_layout::PagedDocument>()?;
                let bytes = world.result(typst_pdf::pdf(&doc, &self.pdf_options()))?;
                write_output(output, &bytes)?;
            }
            "bundle" => {
                let bundle = world.compile::<typst_bundle::Bundle>()?;
                let options = typst_bundle::BundleOptions {
                    pdf: self.pdf_options(),
                    ..Default::default()
                };
                let files = world.result(typst_bundle::export(&bundle, &options))?;
                // Validate every destination before writing any of the bundle.
                let paths = files
                    .keys()
                    .map(|path| bundle_output_path(output, Path::new(path.get_without_slash())))
                    .collect::<Result<Vec<_>>>()?;
                for (path, data) in paths.iter().zip(files.values()) {
                    write_output(path, data)?;
                }
            }
            _ => bail!("unsupported Typst output format: {format}"),
        }
        let dependencies = world
            .dependencies
            .lock()
            .expect("dependencies poisoned")
            .clone();
        Ok(dependencies)
    }

    fn pdf_options(&self) -> typst_pdf::PdfOptions {
        use chrono::{Datelike, Timelike};
        let now = self
            .timestamp
            .and_then(|value| chrono::DateTime::from_timestamp(value, 0))
            .unwrap_or_else(chrono::Utc::now);
        let date = Datetime::from_ymd_hms(
            now.year(),
            now.month() as u8,
            now.day() as u8,
            now.hour() as u8,
            now.minute() as u8,
            now.second() as u8,
        );
        typst_pdf::PdfOptions {
            timestamp: date.map(typst_pdf::Timestamp::new_utc),
            ..Default::default()
        }
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // Bound Typst's global memoization cache across watch rebuilds.
        comemo::evict(10);
    }
}

fn env_path(name: &str, root: &Path) -> Option<PathBuf> {
    std::env::var_os(name).map(|path| root.join(path))
}

fn env_bool(name: &str) -> Result<bool> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .with_context(|| format!("{name} must be true or false")),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(err) => Err(err).with_context(|| format!("invalid {name}")),
    }
}

pub fn features(value: &str) -> Result<Features> {
    value
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| match part.trim() {
            "html" => Ok(Feature::Html),
            "bundle" => Ok(Feature::Bundle),
            "a11y-extras" => Ok(Feature::A11yExtras),
            other => {
                bail!("unknown Typst feature {other:?}; expected html, bundle, or a11y-extras")
            }
        })
        .collect()
}

struct CompileWorld<'a> {
    compiler: &'a Compiler,
    main: FileId,
    library: LazyHash<Library>,
    files: FileStore<ProjectFiles<'a>>,
    time: Time,
    dependencies: Arc<Mutex<Dependencies>>,
    overlay: Option<Source>,
}

impl<'a> CompileWorld<'a> {
    fn new(
        compiler: &'a Compiler,
        input: &Path,
        flags: &str,
        inputs: &[(&str, &str)],
    ) -> Result<Self> {
        let input = crate::util::normalize_path(input)?;
        let path = VirtualPath::virtualize(&compiler.root, &input)?;
        let main = RootedPath::new(VirtualRoot::Project, path).intern();
        let inputs: Dict = inputs
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into_value()))
            .collect();
        let mut library = Library::builder()
            .with_inputs(inputs)
            .with_features(features(flags)?)
            .build();
        library.global.scope_mut().define(
            "__typage_values",
            compiler.metadata.read().expect("metadata poisoned").clone(),
        );
        let time = match compiler.timestamp {
            Some(timestamp) => {
                Time::fixed_timestamp(timestamp).map_err(|err| anyhow::anyhow!(err))?
            }
            None => Time::system(),
        };
        let dependencies = Arc::new(Mutex::new(
            compiler
                .metadata_dependencies
                .read()
                .expect("metadata dependencies poisoned")
                .clone(),
        ));
        Ok(Self {
            compiler,
            main,
            library: LazyHash::new(library),
            files: FileStore::new(ProjectFiles(compiler, dependencies.clone())),
            time,
            dependencies,
            overlay: None,
        })
    }

    fn compile<T: typst::foundations::Output>(&self) -> Result<T> {
        let result = typst::compile::<T>(self);
        if !result.warnings.is_empty() {
            eprint!("{}", self.diagnostics(&result.warnings));
        }
        self.result(result.output)
    }

    fn result<T>(&self, result: SourceResult<T>) -> Result<T> {
        result.map_err(|errors| anyhow::anyhow!(self.diagnostics(&errors)))
    }

    fn diagnostics(&self, diagnostics: &[SourceDiagnostic]) -> String {
        let mut buffer = Buffer::no_color();
        if diagnostics::emit(&mut buffer, self, diagnostics, DiagnosticFormat::Human).is_ok() {
            String::from_utf8_lossy(buffer.as_slice()).into_owned()
        } else {
            diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

impl World for CompileWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }
    fn book(&self) -> &LazyHash<FontBook> {
        self.compiler.fonts().book()
    }
    fn main(&self) -> FileId {
        self.main
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main {
            if let Some(source) = &self.overlay {
                return Ok(source.clone());
            }
        }
        self.files.source(id)
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.file(id)
    }
    fn font(&self, index: usize) -> Option<Font> {
        self.compiler.fonts().font(index)
    }
    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.time.today(offset)
    }
}

impl DiagnosticWorld for CompileWorld<'_> {
    fn name(&self, id: FileId) -> String {
        match id.root() {
            VirtualRoot::Project => self
                .compiler
                .root
                .join(id.vpath().get_without_slash())
                .display()
                .to_string(),
            VirtualRoot::Package(spec) => format!("{spec}{}", id.vpath().get_with_slash()),
        }
    }
}

struct ProjectFiles<'a>(&'a Compiler, Arc<Mutex<Dependencies>>);

impl FileLoader for ProjectFiles<'_> {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        let root = match id.root() {
            VirtualRoot::Project => FsRoot::new(self.0.root.clone()),
            VirtualRoot::Package(spec) => {
                if let Some(root) = self.0.local_packages.obtain(spec) {
                    root
                } else {
                    self.0
                        .packages
                        .lock()
                        .expect("package loader poisoned")
                        .obtain(spec)?
                }
            }
        };
        // FsRoot confines lexical paths. Also prevent symlinks from escaping
        // the project or a package's own root.
        let path = root.resolve(id.vpath())?;
        self.1
            .lock()
            .expect("dependencies poisoned")
            .insert(path.clone(), None);
        let canonical = path
            .canonicalize()
            .map_err(|err| FileError::from_io(err, &path))?;
        let root_path = root
            .path()
            .canonicalize()
            .map_err(|err| FileError::from_io(err, root.path()))?;
        if !canonical.starts_with(&root_path) {
            return Err(FileError::AccessDenied);
        }
        if !canonical.is_file() {
            return Err(FileError::IsDirectory);
        }
        let bytes = fs::read(&canonical).map_err(|err| FileError::from_io(err, &path))?;
        self.1
            .lock()
            .expect("dependencies poisoned")
            .insert(path, Some(fingerprint(&bytes)));
        Ok(Bytes::new(bytes))
    }
}

fn bundle_output_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe bundle output path: {}", relative.display());
    }
    let path = root.join(relative);
    crate::util::check_output_ancestors(&path)?;
    Ok(path)
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::util::write_bytes_if_changed(path, bytes)
}

#[cfg(test)]

pub(crate) fn prepare_output_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::remove_file(path)?,
        Ok(meta) if !meta.is_file() => {
            bail!("refusing to overwrite non-file output {}", path.display())
        }
        Ok(_) => (),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Compiler) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut compiler = Compiler::new(&root, root.join("packages")).unwrap();
        // These tests exercise the bundled fallback without depending on host fonts.
        compiler.ignore_system_fonts = true;
        compiler.ignore_embedded_fonts = false;
        (temp, compiler)
    }

    #[test]
    fn compiles_parallel_inputs_and_reloads_imports_and_assets() {
        let (_temp, compiler) = project();
        let root = &compiler.root;
        fs::write(root.join("helper.typ"), "#let title = [Imported]").unwrap();
        fs::write(root.join("data.txt"), "First asset").unwrap();
        fs::write(root.join("image.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"10\" height=\"10\" fill=\"red\"/></svg>").unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        let input = root.join("nested/main.typ");
        fs::write(&input, "#import \"/helper.typ\": title\n#title #read(\"../data.txt\") #sys.inputs.at(\"name\") #image(\"../image.svg\")").unwrap();
        std::thread::scope(|scope| {
            for name in ["First page", "Second page"] {
                let compiler = &compiler;
                let input = &input;
                scope.spawn(move || {
                    let output = compiler.root.join(format!("{name}.html"));
                    compiler
                        .compile(input, &output, "html", "html", &[("name", name)])
                        .unwrap();
                    let html = fs::read_to_string(output).unwrap();
                    assert!(html.contains("Imported"));
                    assert!(html.contains("First asset"));
                    assert!(html.contains(name));
                    assert!(!html.contains(if name == "First page" {
                        "Second page"
                    } else {
                        "First page"
                    }));
                    assert!(html.contains("<img"));
                });
            }
        });
        fs::write(root.join("helper.typ"), "#let title = [Changed import]").unwrap();
        fs::write(root.join("data.txt"), "Changed asset").unwrap();
        let output = root.join("updated.html");
        compiler
            .compile(&input, &output, "html", "html", &[("name", "Updated")])
            .unwrap();
        let html = fs::read_to_string(output).unwrap();
        assert!(html.contains("Changed import"));
        assert!(html.contains("Changed asset"));
        assert!(!html.contains("First asset"));
    }

    #[test]
    fn loads_local_packages_and_exports_pdf_and_bundle() {
        let (_temp, compiler) = project();
        let root = &compiler.root;
        let pkg = root.join("packages/local/example/0.1.0");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("typst.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nentrypoint = \"lib.typ\"\n",
        )
        .unwrap();
        fs::write(pkg.join("lib.typ"), "#let greeting = [Package content]").unwrap();
        let input = root.join("main.typ");
        fs::write(
            &input,
            "#import \"@local/example:0.1.0\": greeting\n#greeting",
        )
        .unwrap();
        let pdf = root.join("print.pdf");
        compiler.compile(&input, &pdf, "pdf", "", &[]).unwrap();
        assert!(fs::read(pdf).unwrap().starts_with(b"%PDF-"));
        fs::write(&input, "#import \"@local/example:0.1.0\": greeting\n#document(\"index.html\")[#greeting]\n#document(\"nested/page.html\")[Second]\n#document(\"print.pdf\")[#greeting]\n#asset(\"asset.txt\", bytes(\"Asset\"))").unwrap();
        let bundle = root.join("bundle");
        compiler
            .compile(&input, &bundle, "bundle", "bundle,html", &[])
            .unwrap();
        assert!(fs::read_to_string(bundle.join("index.html"))
            .unwrap()
            .contains("Package content"));
        assert!(fs::read_to_string(bundle.join("nested/page.html"))
            .unwrap()
            .contains("Second"));
        assert!(fs::read(bundle.join("print.pdf"))
            .unwrap()
            .starts_with(b"%PDF-"));
        assert_eq!(fs::read(bundle.join("asset.txt")).unwrap(), b"Asset");
    }

    #[test]
    fn resolves_cached_packages_without_network() {
        let (_temp, compiler) = project();
        let root = &compiler.root;
        let pkg = root.join("cache/preview/example/0.1.0");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("typst.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nentrypoint = \"lib.typ\"\n",
        )
        .unwrap();
        fs::write(pkg.join("lib.typ"), "#let greeting = [Cached package]").unwrap();
        *compiler.packages.lock().unwrap() = SystemPackages::from_parts(
            None,
            Some(FsPackages::new(root.join("cache"))),
            UniversePackages::new(SystemDownloader::new("typage-test")),
        );
        let input = root.join("main.typ");
        fs::write(
            &input,
            "#import \"@preview/example:0.1.0\": greeting\n#greeting",
        )
        .unwrap();
        let output = root.join("index.html");
        compiler
            .compile(&input, &output, "html", "html", &[])
            .unwrap();
        assert!(fs::read_to_string(output)
            .unwrap()
            .contains("Cached package"));
    }

    #[test]
    fn reports_source_locations_and_validates_features() {
        let (_temp, compiler) = project();
        let input = compiler.root.join("broken.typ");
        fs::write(&input, "Hello\n#unknown_function()\n").unwrap();
        let output = compiler.root.join("index.html");
        let err = compiler
            .compile(&input, &output, "html", "html", &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("broken.typ:2:"), "{err}");
        assert!(err.contains("unknown variable"), "{err}");
        assert!(!output.exists());
        assert!(features("html, bundle").is_ok());
        assert!(features("unknown").is_err());
        fs::write(&input, "Hello").unwrap();
        assert!(compiler.compile(&input, &output, "html", "", &[]).is_err());
    }

    #[test]
    fn fixed_timestamp_controls_today_and_pdf_metadata() {
        let (_temp, mut compiler) = project();
        compiler.timestamp = Some(0);
        let input = compiler.root.join("main.typ");
        fs::write(&input, "#datetime.today().display()").unwrap();
        let output = compiler.root.join("index.html");
        compiler
            .compile(&input, &output, "html", "html", &[])
            .unwrap();
        assert!(fs::read_to_string(output).unwrap().contains("1970"));
        assert!(compiler.pdf_options().timestamp.is_some());
    }

    #[test]
    fn rejects_bundle_path_traversal() {
        let (_temp, compiler) = project();
        for path in ["../outside", "/absolute", "", "a/../../outside"] {
            assert!(bundle_output_path(&compiler.root, Path::new(path)).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_read_and_write_symlink_escapes() {
        use std::os::unix::fs::symlink;
        let (_temp, compiler) = project();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(outside.path(), compiler.root.join("escape")).unwrap();
        let input = compiler.root.join("main.typ");
        fs::write(&input, "#read(\"escape/secret.txt\")").unwrap();
        let err = compiler
            .compile(&input, &compiler.root.join("out.html"), "html", "html", &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("access denied"), "{err}");
        assert!(bundle_output_path(&compiler.root, Path::new("escape/out.html")).is_err());
        assert!(write_output(&compiler.root.join("escape/out.html"), b"no").is_err());
        assert!(!outside.path().join("out.html").exists());
        let output = compiler.root.join("leaf.html");
        symlink(outside.path().join("secret.txt"), &output).unwrap();
        write_output(&output, b"new").unwrap();
        assert_eq!(
            fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "secret"
        );
        assert_eq!(fs::read(&output).unwrap(), b"new");
    }
}
