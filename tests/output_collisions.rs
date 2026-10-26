use std::{fs, process::Command};

#[test]
fn rejects_collisions_before_publishing_even_without_print_documents() {
    for relative in [
        "index.html",
        "about",
        "feed.xml",
        "robots.txt",
        "search_index.json",
    ] {
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
        cfg["feed"] = true.into();
        cfg["robots"] = true.into();
        cfg.as_table_mut()
            .unwrap()
            .insert("feed_path".into(), "feed.xml".into());
        cfg["search"]["enabled"] = true.into();
        fs::write(config, toml::to_string(&cfg).unwrap()).unwrap();
        fs::write(root.join("static").join(relative), "collision").unwrap();
        for command in ["build", "doctor", "bundle"] {
            let result = Command::new(env!("CARGO_BIN_EXE_typage"))
                .args([command, "--root"])
                .arg(root)
                .output()
                .unwrap();
            let message = format!(
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(!result.status.success(), "{command}: {relative}: {message}");
            assert!(message.contains("output collision"), "{message}");
        }
        assert!(!root.join("public/index.html").exists());
    }
}
