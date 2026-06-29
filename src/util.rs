use std::ffi::OsStr;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub fn normalize_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        fs::canonicalize(path).with_context(|| format!("failed to canonicalize {}", path.display()))
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        let from = entry.path();
        let rel = from.strip_prefix(src)?;
        let to = dst.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&to)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_file_if_changed(from, &to)?;
        }
    }
    Ok(())
}

fn copy_file_if_changed(from: &Path, to: &Path) -> Result<()> {
    let src_meta = fs::symlink_metadata(from)?;
    if !src_meta.file_type().is_file() {
        return Ok(());
    }
    match fs::symlink_metadata(to) {
        Ok(dst_meta) => {
            let dst_type = dst_meta.file_type();
            if dst_type.is_symlink() {
                fs::remove_file(to)
                    .with_context(|| format!("failed to remove stale symlink {}", to.display()))?;
            } else if dst_type.is_file() {
                if src_meta.len() == dst_meta.len() && files_equal(from, to)? {
                    return Ok(());
                }
            } else {
                bail!(
                    "refusing to overwrite non-file static output {}",
                    to.display()
                );
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect static output {}", to.display()));
        }
    }
    fs::copy(from, to)
        .with_context(|| format!("failed to copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn files_equal(a: &Path, b: &Path) -> Result<bool> {
    const BUF_SIZE: usize = 64 * 1024;
    let mut left = BufReader::with_capacity(BUF_SIZE, fs::File::open(a)?);
    let mut right = BufReader::with_capacity(BUF_SIZE, fs::File::open(b)?);
    let mut a_buf = [0u8; BUF_SIZE];
    let mut b_buf = [0u8; BUF_SIZE];
    loop {
        let a_len = left.read(&mut a_buf)?;
        let b_len = right.read(&mut b_buf)?;
        if a_len != b_len {
            return Ok(false);
        }
        if a_len == 0 {
            return Ok(true);
        }
        if &a_buf[..a_len] != &b_buf[..b_len] {
            return Ok(false);
        }
    }
}

pub fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

pub fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                fs::remove_file(path).with_context(|| {
                    format!("failed to remove stale symlink {}", path.display())
                })?;
            } else if file_type.is_file() {
                if fs::read_to_string(path).unwrap_or_default() == contents {
                    return Ok(());
                }
            } else {
                bail!("refusing to overwrite non-file path {}", path.display());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

pub fn hash_strs(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if ch.is_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "section".to_string()
    } else {
        trimmed
    }
}

pub fn typst_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn typst_opt_string(s: &Option<String>) -> String {
    match s {
        Some(s) => typst_string(s),
        None => "none".to_string(),
    }
}

pub fn typst_array_str(values: &[String]) -> String {
    let items = values.iter().map(|v| typst_string(v)).collect::<Vec<_>>();
    typst_tuple(items)
}

pub fn typst_tuple(items: Vec<String>) -> String {
    match items.len() {
        0 => "()".to_string(),
        1 => format!("({},)", items[0]),
        _ => format!("({})", items.join(", ")),
    }
}

pub fn to_posix_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            Component::ParentDir => Some("..".to_string()),
            Component::CurDir => None,
            Component::RootDir => Some(String::new()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_string()),
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn relative_path(from_dir: &Path, to: &Path) -> Result<PathBuf> {
    let from = fs::canonicalize(from_dir)
        .with_context(|| format!("failed to canonicalize {}", from_dir.display()))?;
    let to =
        fs::canonicalize(to).with_context(|| format!("failed to canonicalize {}", to.display()))?;
    let from_parts = path_parts(&from);
    let to_parts = path_parts(&to);
    let mut common = 0usize;
    while common < from_parts.len()
        && common < to_parts.len()
        && from_parts[common] == to_parts[common]
    {
        common += 1;
    }
    let mut rel = PathBuf::new();
    for _ in common..from_parts.len() {
        rel.push("..");
    }
    for part in &to_parts[common..] {
        rel.push(part);
    }
    Ok(rel)
}

fn path_parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_string()),
            Component::RootDir => Some("/".to_string()),
            _ => None,
        })
        .collect()
}

pub fn is_typst_file(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("typ"))
}

pub fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .map(|s| s.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let val = u8::from_str_radix(hex, 16).ok()?;
            out.push(val);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn copy_dir_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp =
            std::env::temp_dir().join(format!("typage-copy-link-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("ok.txt"), "ok").unwrap();
        fs::write(tmp.join("outside.txt"), "outside").unwrap();
        symlink(tmp.join("outside.txt"), src.join("leak.txt")).unwrap();

        copy_dir(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("ok.txt")).unwrap(), "ok");
        assert!(!dst.join("leak.txt").exists());
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_replaces_destination_symlink_with_regular_file() {
        use std::os::unix::fs::symlink;

        let tmp =
            std::env::temp_dir().join(format!("typage-copy-dst-link-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("asset.txt"), "asset").unwrap();
        fs::write(tmp.join("outside.txt"), "outside").unwrap();
        symlink(tmp.join("outside.txt"), dst.join("asset.txt")).unwrap();

        copy_dir(&src, &dst).unwrap();
        let meta = fs::symlink_metadata(dst.join("asset.txt")).unwrap();
        assert!(meta.file_type().is_file());
        assert!(!meta.file_type().is_symlink());
        assert_eq!(fs::read_to_string(dst.join("asset.txt")).unwrap(), "asset");
        assert_eq!(
            fs::read_to_string(tmp.join("outside.txt")).unwrap(),
            "outside"
        );
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_replaces_dangling_destination_symlink_with_regular_file() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join(format!(
            "typage-copy-dangling-link-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        let outside = tmp.join("outside");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(src.join("asset.txt"), "asset").unwrap();
        let target = outside.join("missing.txt");
        symlink(&target, dst.join("asset.txt")).unwrap();

        copy_dir(&src, &dst).unwrap();
        let meta = fs::symlink_metadata(dst.join("asset.txt")).unwrap();
        assert!(meta.file_type().is_file());
        assert!(!meta.file_type().is_symlink());
        assert_eq!(fs::read_to_string(dst.join("asset.txt")).unwrap(), "asset");
        assert!(!target.exists());
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[test]
    fn write_if_changed_replaces_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let tmp =
            std::env::temp_dir().join(format!("typage-write-link-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let outside = tmp.join("outside.txt");
        let link = tmp.join("out.txt");
        fs::write(&outside, "outside").unwrap();
        symlink(&outside, &link).unwrap();

        write_if_changed(&link, "new").unwrap();
        let meta = fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_file());
        assert!(!meta.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&link).unwrap(), "new");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
        let _ = fs::remove_dir_all(tmp);
    }

    #[cfg(unix)]
    #[test]
    fn write_if_changed_replaces_dangling_symlink_without_creating_target() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join(format!(
            "typage-write-dangling-link-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let outside = tmp.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("missing.txt");
        let link = tmp.join("out.txt");
        symlink(&target, &link).unwrap();

        write_if_changed(&link, "new").unwrap();
        let meta = fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_file());
        assert!(!meta.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&link).unwrap(), "new");
        assert!(!target.exists());
        let _ = fs::remove_dir_all(tmp);
    }
}
