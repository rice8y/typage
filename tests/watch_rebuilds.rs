use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(label: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "watch rebuild timed out: {label}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn contains(path: &Path, text: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|contents| contents.contains(text))
}

#[test]
fn rebuilds_after_bursts_config_changes_and_missing_directory_creation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    fs::remove_dir_all(root.join("static")).unwrap();
    let _child = Running(
        Command::new(env!("CARGO_BIN_EXE_typage"))
            .args(["watch", "--no-hooks", "--root"])
            .arg(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let page = root.join("content/about.typ");
    let original = fs::read_to_string(&page).unwrap();
    let html = root.join("public/about/index.html");
    wait_for("initial", || html.exists());
    for text in ["First edit", "Second edit", "Final saved content"] {
        fs::write(&page, format!("{original}\n{text}\n")).unwrap();
        thread::sleep(Duration::from_millis(75));
    }
    wait_for("burst", || contains(&html, "Final saved content"));
    fs::create_dir_all(root.join("static")).unwrap();
    fs::write(root.join("static/new.txt"), "New directory asset").unwrap();
    wait_for("new directory", || {
        contains(&root.join("public/new.txt"), "New directory asset")
    });
    let config = root.join("config.toml");
    let mut cfg: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    cfg["out_dir"] = "dist".into();
    fs::write(config, toml::to_string(&cfg).unwrap()).unwrap();
    wait_for("config", || {
        contains(&root.join("dist/about/index.html"), "Final saved content")
    });
}

#[cfg(unix)]
#[test]
fn preserves_changes_during_hooks_and_honors_no_hooks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(Command::new(env!("CARGO_BIN_EXE_typage"))
        .arg("init")
        .arg(root)
        .output()
        .unwrap()
        .status
        .success());
    let config = root.join("config.toml");
    let mut cfg: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    cfg["hooks"].as_table_mut().unwrap().insert(
        "post_build".into(),
            "sh -c 'touch hook-started; sleep 1; touch hook-finished'".into(),
    );
    fs::write(&config, toml::to_string(&cfg).unwrap()).unwrap();
    {
        let _child = Running(
            Command::new(env!("CARGO_BIN_EXE_typage"))
                .args(["watch", "--root"])
                .arg(root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        wait_for("post hook", || root.join("hook-started").exists());
        let page = root.join("content/about.typ");
        let source = fs::read_to_string(&page).unwrap();
        fs::write(page, format!("{source}\nChanged during build hook\n")).unwrap();
        wait_for("queued change", || {
            contains(
                &root.join("public/about/index.html"),
                "Changed during build hook",
            )
        });
        wait_for("hook completion", || root.join("hook-finished").exists());
    }
    fs::remove_file(root.join("hook-started")).unwrap();
    let _child = Running(
        Command::new(env!("CARGO_BIN_EXE_typage"))
            .args(["watch", "--no-hooks", "--root"])
            .arg(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    thread::sleep(Duration::from_secs(2));
    assert!(!root.join("hook-started").exists());
}
