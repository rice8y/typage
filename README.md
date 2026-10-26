# Typage

An experimental static site generator with Typst content and templates, powered by an embedded Typst compiler.

> **Do not use Typage for production use cases.**
> Typst's HTML export is under active development and incomplete. Typage is experimental too; expect breaking changes and output changes. See the [upstream tracking issue](https://github.com/typst/typst/issues/5512).

## Requirements

- Rust 1.92 or later, Cargo, and a native linker to build or install Typage.
- On Linux, OpenSSL development headers and `pkg-config` for HTTPS package downloads.

## Quick Start

Install from this repository:

```sh
cargo install --git https://github.com/rice8y/typage --locked
typage init my-site
cd my-site
typage dev
```

For a local checkout, use `cargo install --path . --locked`.

The starter includes a home page, an about page, a posts collection, templates, and a stylesheet. Feeds, search, and taxonomies are opt-in.

```text
config.toml    Site configuration
content/       Typst pages and collection schemas
templates/     Page, list, and print templates
static/        Assets copied as-is
public/        Generated site
.typage/       Build cache and generated packages
```

Treat `public/` and `.typage/` as disposable. Keep source files and assets outside them.

## Write Content

Create a page:

```sh
typage new posts/first-post --title "Hello"
```

Edit `content/posts/first-post.typ`:

```typst
#show: page.with(
  title: "Hello",
  description: "My first post.",
)

= Welcome

Written in Typst, published as HTML.
```

Typage reads and removes the metadata directive before compilation; no import is required. `project.with(...)` is an alias. Define collection-specific fields in `content/config.typ`. TOML front matter remains supported for legacy content.

By default, `content/posts/first-post.typ` becomes `/posts/first-post/`. See [Content](docs/content.md) for schemas, routing, and publishing options.

## Build and Export

```sh
typage dev             # Serve with live reload
typage doctor          # Check configuration, routes, and links
typage build           # Build into public/
typage build --pdf     # Also create per-page PDFs
typage build --force   # Rebuild all outputs
```

To combine pages into a single PDF, add this to `config.toml`:

```toml
[[pdf_documents]]
path = "print.pdf"
title = "My Site"
template = "print.typ"
sections = ["posts"]
sort_by = "date_desc"
```

A normal `typage build` then produces `public/print.pdf` using the starter's print template. See [Building and PDF Export](docs/building.md) for custom print templates, compiler settings, caching, and profiling.

## Extend and Deploy

Use `typage init my-site --starter github:owner/repo` for an external starter, or `typage add` to apply one to an existing project. Themes provide templates and components; plugins generate content before the build.

Generate hosting configuration with `typage deploy init <target>`, where `<target>` is `github-pages`, `cloudflare-pages`, `netlify`, or `vercel`. Typage generates deployment files; the hosting platform publishes the site.

## Documentation

| Guide | Topics |
| --- | --- |
| [Content](docs/content.md) | Metadata, schemas, permalinks, feeds, search |
| [Templates and Themes](docs/templates.md) | Components, theme configuration, site API |
| [Starters and Plugins](docs/extensions.md) | Starter manifests, plugin installation and protocol |
| [Building and PDF Export](docs/building.md) | Compiler settings, PDFs, parallel builds, scripts, hooks |
| [Deployment](docs/deployment.md) | Hosting scaffolds and CI setup |
| [Development](docs/development.md) | Code style, module responsibilities, verification |

Run `typage --help` or `typage <command> --help` for command options. See [config.toml](config.toml) for configuration examples.

## License

This project is distributed under the MIT License. See [LICENSE](LICENSE) for details.
