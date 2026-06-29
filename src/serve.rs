use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};

use crate::build::{self, BuildOptions};
use crate::config::load_config;
use crate::util::{normalize_path, percent_decode};

const LIVE_JS: &str = r#"(() => {
  const url = '/__typage/revision';
  let current = null;
  async function tick() {
    try {
      const res = await fetch(url, { cache: 'no-store' });
      if (!res.ok) return;
      const next = (await res.text()).trim();
      if (current === null) current = next;
      else if (next !== current) location.reload();
    } catch (_) {}
  }
  setInterval(tick, 1200);
  tick();
})();
"#;

const SECURITY_HEADERS: &[(&str, &str)] = &[
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "strict-origin-when-cross-origin"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    (
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=()",
    ),
];

pub fn serve(
    root: PathBuf,
    addr: String,
    live_reload: bool,
    drafts: bool,
    pdf: bool,
    jobs: Option<usize>,
) -> Result<()> {
    let root = normalize_path(&root)?;
    let cfg = load_config(&root)?;
    let public = fs::canonicalize(root.join(cfg.out_dir))
        .with_context(|| "failed to canonicalize output directory before serving")?;
    let revision = Arc::new(AtomicU64::new(1));
    if live_reload {
        start_live_rebuild_thread(root.clone(), revision.clone(), drafts, pdf, jobs);
    };
    let listener = TcpListener::bind(&addr)?;
    println!(
        "serving {} at http://{}{}",
        public.display(),
        addr,
        if live_reload { " with live reload" } else { "" }
    );
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let public = public.clone();
                let revision = revision.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_client(stream, &public, live_reload, &revision) {
                        eprintln!("warning: request failed: {err}");
                    }
                });
            }
            Err(err) => eprintln!("warning: connection failed: {err}"),
        }
    }
    Ok(())
}

fn start_live_rebuild_thread(
    root: PathBuf,
    revision: Arc<AtomicU64>,
    drafts: bool,
    pdf: bool,
    jobs: Option<usize>,
) {
    thread::spawn(move || {
        let cfg = match load_config(&root) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("live reload disabled: failed to load config: {err:?}");
                return;
            }
        };
        let (tx, rx) = channel();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) {
            Ok(w) => w,
            Err(err) => {
                eprintln!("live reload disabled: failed to create watcher: {err}");
                return;
            }
        };
        for path in [&cfg.content_dir, &cfg.templates_dir, &cfg.static_dir] {
            let full = root.join(path);
            if full.exists() {
                if let Err(err) = watcher.watch(&full, RecursiveMode::Recursive) {
                    eprintln!("warning: failed to watch {}: {err}", full.display());
                }
            }
        }
        if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
            let full = root.join("themes").join(theme);
            if full.exists() {
                if let Err(err) = watcher.watch(&full, RecursiveMode::Recursive) {
                    eprintln!("warning: failed to watch {}: {err}", full.display());
                }
            }
        }
        let mut last = Instant::now() - Duration::from_secs(10);
        loop {
            match rx.recv() {
                Ok(Ok(_event)) => {
                    if last.elapsed() < Duration::from_millis(250) {
                        continue;
                    }
                    last = Instant::now();
                    println!("change detected; rebuilding");
                    let opts = BuildOptions {
                        root: root.clone(),
                        drafts,
                        force: false,
                        typst_override: None,
                        pdf,
                        keep_going: true,
                        jobs,
                        profile: false,
                        explain: false,
                        quiet: false,
                        verbose: false,
                    };
                    match build::build_site(&opts) {
                        Ok(_) => {
                            revision.fetch_add(1, Ordering::SeqCst);
                            println!("live reload revision updated");
                        }
                        Err(err) => eprintln!("build failed: {err:?}"),
                    }
                }
                Ok(Err(err)) => eprintln!("watch error: {err}"),
                Err(err) => {
                    eprintln!("watch channel closed: {err}");
                    return;
                }
            }
        }
    });
}

fn handle_client(
    mut stream: TcpStream,
    public: &Path,
    live_reload: bool,
    revision: &Arc<AtomicU64>,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut parts = req.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("");
    let uri = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return respond(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            b"Method Not Allowed",
        );
    };
    if live_reload && uri.split('?').next().unwrap_or("") == "/__typage/revision" {
        let body = revision.load(Ordering::SeqCst).to_string();
        return respond_no_cache(
            &mut stream,
            200,
            "OK",
            "text/plain; charset=utf-8",
            body.as_bytes(),
        );
    }
    if live_reload && uri.split('?').next().unwrap_or("") == "/__typage/live.js" {
        return respond_no_cache(
            &mut stream,
            200,
            "OK",
            "text/javascript; charset=utf-8",
            LIVE_JS.as_bytes(),
        );
    }
    let Some(mut path) = candidate_public_path(public, uri) else {
        return respond(
            &mut stream,
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"Bad Request",
        );
    };
    if path.is_dir() {
        path = path.join("index.html");
    }
    let Some(path) = resolve_public_file(public, &path)? else {
        return respond(
            &mut stream,
            404,
            "Not Found",
            "text/plain; charset=utf-8",
            b"Not Found",
        );
    };
    let mime = mime_type(&path);
    let cacheable = !(live_reload && mime.starts_with("text/html"));
    let etag = if cacheable {
        file_etag(&path).ok()
    } else {
        None
    };
    if cacheable {
        if let (Some(req_etag), Some(etag)) = (header_value(&req, "if-none-match"), etag.as_deref())
        {
            if req_etag == etag {
                return respond_not_modified(&mut stream, etag);
            }
        }
    }
    if method == "HEAD" {
        let len = fs::metadata(&path)?.len();
        return respond_header(&mut stream, 200, "OK", mime, len, etag.as_deref());
    }
    if live_reload && mime.starts_with("text/html") {
        let body = inject_live_reload(fs::read(&path)?);
        respond(&mut stream, 200, "OK", mime, &body)
    } else {
        respond_file(&mut stream, &path, mime, etag.as_deref())
    }
}

fn inject_live_reload(mut body: Vec<u8>) -> Vec<u8> {
    let script = br#"<script src="/__typage/live.js" defer></script>"#;
    let lower = String::from_utf8_lossy(&body).to_lowercase();
    if let Some(pos) = lower.rfind("</body>") {
        body.splice(pos..pos, script.iter().copied());
        body
    } else {
        body.extend_from_slice(script);
        body
    }
}

fn candidate_public_path(public: &Path, uri: &str) -> Option<PathBuf> {
    let uri = uri.split('?').next().unwrap_or("/");
    let decoded = percent_decode(uri)?;
    if decoded.contains('\0') {
        return None;
    }
    let mut path = public.to_path_buf();
    for part in decoded.trim_start_matches('/').split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\\') {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

fn resolve_public_file(public: &Path, candidate: &Path) -> Result<Option<PathBuf>> {
    if !candidate.exists() {
        return Ok(None);
    }
    let path = fs::canonicalize(candidate).with_context(|| {
        format!(
            "failed to canonicalize requested path {}",
            candidate.display()
        )
    })?;
    if !path.starts_with(public) {
        return Ok(None);
    }
    let meta = fs::metadata(&path)?;
    if !meta.is_file() {
        return Ok(None);
    }
    Ok(Some(path))
}

fn header_value(req: &str, name: &str) -> Option<String> {
    for line in req.lines().skip(1) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(name) {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn file_etag(path: &Path) -> Result<String> {
    let meta = fs::metadata(path)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(format!("W/\"{:x}-{:x}\"", meta.len(), modified))
}

fn respond_not_modified(stream: &mut TcpStream, etag: &str) -> Result<()> {
    write!(stream, "HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\n")?;
    write_security_headers(stream)?;
    write!(stream, "Connection: close\r\n\r\n")?;
    Ok(())
}

fn respond_header(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    content_type: &str,
    content_length: u64,
    etag: Option<&str>,
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\n"
    )?;
    if let Some(etag) = etag {
        write!(stream, "ETag: {etag}\r\n")?;
    }
    write_security_headers(stream)?;
    write!(stream, "Connection: close\r\n\r\n")?;
    Ok(())
}

fn respond_file(
    stream: &mut TcpStream,
    path: &Path,
    content_type: &str,
    etag: Option<&str>,
) -> Result<()> {
    let len = fs::metadata(path)?.len();
    respond_header(stream, 200, "OK", content_type, len, etag)?;
    let mut file = BufReader::new(File::open(path)?);
    io::copy(&mut file, stream)?;
    Ok(())
}

fn respond(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    write_security_headers(stream)?;
    write!(stream, "Connection: close\r\n\r\n")?;
    stream.write_all(body)?;
    Ok(())
}

fn respond_no_cache(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    write_security_headers(stream)?;
    write!(stream, "Connection: close\r\n\r\n")?;
    stream.write_all(body)?;
    Ok(())
}

fn write_security_headers(stream: &mut TcpStream) -> Result<()> {
    for (name, value) in SECURITY_HEADERS {
        write!(stream, "{name}: {value}\r\n")?;
    }
    Ok(())
}

fn mime_type(path: &Path) -> &'static str {
    match path.file_name().and_then(OsStr::to_str).unwrap_or("") {
        "feed.xml" | "rss.xml" => return "application/rss+xml; charset=utf-8",
        "atom.xml" => return "application/atom+xml; charset=utf-8",
        "sitemap.xml" => return "application/xml; charset=utf-8",
        _ => {}
    }
    match path.extension().and_then(OsStr::to_str).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_public_path_rejects_traversal_and_bad_encoding() {
        let public = Path::new("/tmp/site/public");
        assert!(candidate_public_path(public, "/posts/hello/").is_some());
        assert!(candidate_public_path(public, "/../secret.txt").is_none());
        assert!(candidate_public_path(public, "/%2e%2e/secret.txt").is_none());
        assert!(candidate_public_path(public, "/bad%zz").is_none());
        assert!(candidate_public_path(public, "/bad%00name").is_none());
        assert!(candidate_public_path(public, "/bad\\name").is_none());
    }

    #[test]
    fn feed_files_use_feed_mime_types() {
        assert_eq!(
            mime_type(Path::new("feed.xml")),
            "application/rss+xml; charset=utf-8"
        );
        assert_eq!(
            mime_type(Path::new("atom.xml")),
            "application/atom+xml; charset=utf-8"
        );
        assert_eq!(
            mime_type(Path::new("sitemap.xml")),
            "application/xml; charset=utf-8"
        );
    }

    #[test]
    fn resolve_public_file_allows_regular_file_inside_public() {
        let tmp = std::env::temp_dir().join(format!("typage-serve-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let public = tmp.join("public");
        fs::create_dir_all(&public).unwrap();
        let file = public.join("index.html");
        fs::write(&file, "ok").unwrap();
        let public = fs::canonicalize(&public).unwrap();
        let resolved = resolve_public_file(&public, &file).unwrap().unwrap();
        assert_eq!(resolved, fs::canonicalize(&file).unwrap());
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_public_file_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let tmp =
            std::env::temp_dir().join(format!("typage-serve-link-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let public = tmp.join("public");
        fs::create_dir_all(&public).unwrap();
        let secret = tmp.join("secret.txt");
        fs::write(&secret, "secret").unwrap();
        let link = public.join("leak.txt");
        symlink(&secret, &link).unwrap();
        let public = fs::canonicalize(&public).unwrap();
        assert!(resolve_public_file(&public, &link).unwrap().is_none());
        let _ = fs::remove_dir_all(tmp);
    }
}
