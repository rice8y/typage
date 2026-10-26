# Starters and Plugins

Distribute project files as starters and content generators as executable plugins.

## Starters And Presets

Starters are file bundles that can also register plugins. They are meant for templates, static assets, theme files, content schemas, and project presets. Because these files are mostly Typage, Typst, CSS, and static assets, they are usually better distributed as Git repositories than as crates.io packages.

Create a project from a starter:

```sh
typage init my-docs --starter github:rice8y/typage-starter-typst-docs --install-plugins
typage init my-docs --starter https://github.com/rice8y/typage-starter-typst-docs.git
typage init my-docs --starter ../typage-starter-typst-docs
```

Apply a starter or preset to an existing project:

```sh
typage add github:rice8y/typage-starter-typst-docs --install-plugins
typage add ../typage-starter-typst-docs --force
```

Starter repositories can place project files under `files/` and describe plugin registrations in `typage.starter.toml`:

```toml
name = "typst-docs"
description = "Typst package API documentation starter."
min_typage = "0.2.0"

[plugins.typst_docs]
package = "typage-plugin-typst-docs"
version = "0.1.0"
watch = ["package/**/*.typ", "package/typst.toml"]
outputs = ["content/api"]

[plugins.typst_docs.config]
source_dir = "package"
out_dir = "content/api"
template = "typst-docs.typ"
```

The `files/` directory is copied into the project root. Existing files are not overwritten unless `--force` is passed. Plugin registrations are merged into `typage.plugins.toml`; `--install-plugins` also installs declared plugin binaries with Cargo when the plugin entry does not use `command`.

Supported starter sources:

```text
github:owner/repo
github:owner/repo#tag-or-branch
https://github.com/owner/repo.git
git+https://github.com/owner/repo.git
../local-starter
```

## Plugins

Plugins are external executables that can generate or update Typage project files before the normal content discovery step. This keeps Typage as the single CLI entrypoint while allowing domain-specific generators, such as a Typst package API documentation generator, to ship independently.

Install and register a plugin:

```sh
typage plugin install typst-docs
typage plugin install typst-docs --git https://github.com/rice8y/typage-plugin-typst-docs
typage plugin install typst-docs --path ../typage-plugin-typst-docs
typage plugin install typst-docs --command ../tools/typage-plugin-typst-docs
```

By default, `typage plugin install typst-docs` runs:

```sh
cargo install typage-plugin-typst-docs --locked
```

`--git` and `--path` switch the Cargo install source. `--command` skips Cargo and registers an existing executable. The registration is written to `typage.plugins.toml`, which is intended to be committed with the project.

Example registration:

```toml
[plugins.typst_docs]
package = "typage-plugin-typst-docs"
version = "0.1.0"
watch = ["packages/demo/**/*.typ"]
outputs = ["content/api"]

[plugins.typst_docs.config]
source_dir = "packages/demo"
out_dir = "content/api"
```

Plugin entries may also live directly in `config.toml` under `[plugins.<name>]`. If both files define the same plugin name, `typage.plugins.toml` wins.

During `typage build`, each enabled plugin is invoked before Typage reads `content/`:

```sh
<plugin-command> generate --root <project-root> --name <plugin-name>
```

Typage also sets `TYPAGE_ROOT`, `TYPAGE_PLUGIN_NAME`, and `TYPAGE_PLUGIN_CONFIG_JSON`. `plugin.args` are appended after the generated command arguments. `watch` paths are watched by `typage watch` and `typage serve --live-reload`; they must be project-relative paths.

Plugin doctor checks run:

```sh
<plugin-command> manifest --json
```

The manifest must print valid JSON. Typage does not install plugins automatically during build, serve, or deploy; install the plugin binary explicitly in local development and CI.

Starter application preflights all file conflicts and plugin registrations before copying files. Writes use atomic file replacement and reject parent-directory symlinks. Temporary Git checkouts are cleaned up on success or failure. This is not a transaction for external `cargo install` operations or an operating-system I/O failure; review and trust starters and plugins before executing them.
