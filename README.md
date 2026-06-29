# typage

Typage is an experimental static site generator written in Rust and powered by Typst HTML Export.

The goal is to combine the safety and predictable output model of tools like Zola or Jekyll with a Typst-native authoring and templating experience.

## Philosophy

- Treat `public/` as disposable generated output.
- Detect broken links, invalid routes, and alias collisions early.
- Keep content authoring in Typst.
- Keep templates and reusable components in Typst.
- Make the CLI feel familiar to users of npm-style project scripts.

## Features

- Build `content/**/*.typ` into `public/**/index.html`.
- TOML front matter via `serde` and `toml`.
- Per-page `template = "..."` selection.
- Section metadata through `_index.typ`.
- Section and taxonomy pagination through `paginate_by`.
- Sorting through `sort_by`.
- Content fields such as `updated`, `weight`, `expires`, and `[extra]`.
- Permalink policies with `:section`, `:slug`, `:path`, `:filename`, `:year`, `:month`, and `:day`.
- Stable page fields such as `kind`, `slug`, `path`, `file_path`, `canonical_url`, and `aliases`.
- Nested sections with parent, children, ancestors, and siblings.
- Configurable taxonomies such as tags, categories, or authors.
- Internal links like `@/path/to/page.typ` and `@/path/to/page.typ#fragment`.
- Broken internal link detection.
- URL and alias path traversal rejection.
- Page, generated URL, and alias collision detection.
- Table of contents generation from headings.
- Recursive dependency hashing for `#import`, `#include`, `read(...)`, and `image(...)`.
- Incremental build cache.
- Isolated wrappers under `.typage/wrappers/`.
- Stale output cleanup.
- Alias redirect page generation.
- Optional RSS, Atom, sitemap, and robots outputs.
- Canonical URL, Open Graph, and Twitter Card metadata in default templates.
- Full-text `search_index.json` with field scores, heading entries, and CJK-friendly tokenization.
- Theme-owned Typst components through `@local/typage-theme:0.1.0`.
- Built-in shortcodes: `note`, `figure`, and `youtube`.
- `serve --live-reload`.
- Threaded development server with read/write timeouts.
- Streamed static file responses for large assets.
- ETag and 304 responses for cacheable dev-server files.
- Changed-only static asset copying.
- Parallel Typst compile jobs through `jobs = 0` or `--jobs`.
- Job-local page context through `--input typage_current=...`.
- `doctor`, `new`, `theme new`, `theme list --verbose`, `theme info`, and `theme check`.
- Deployment scaffolds for GitHub Pages, Cloudflare Pages, Netlify, and Vercel.
- `run` scripts plus `dev`, `preview`, and `check` aliases.
- Generated local Typst packages `@local/typage:0.1.0` and `@local/typage-theme:0.1.0`.
- Section-local previous and next navigation.
- HTML/PDF dual-target helpers using `context { ... target() ... }`.
- Unit tests plus a Typst-backed integration test when `typst` is available.

## Quick Start

```sh
cargo install --path .

typage init my-site
cd my-site
typage dev
```

`typage init` creates a small general-purpose starter: a home page, an about page, a posts section with one sample article, Typst templates, theme component files, and one stylesheet. It is meant to feel closer to an Astro or Jekyll starter than a blank page, while staying easy to delete or reshape.

Equivalent npm-like form:

```sh
typage run dev
```

## Starter Layout

```text
typage/
|-- config.toml
|-- content/
|   |-- index.typ
|   |-- about.typ
|   `-- posts/
|       |-- _index.typ
|       `-- hello.typ
|-- templates/
|   |-- base.typ
|   |-- list.typ
|   |-- helpers.typ
|   `-- components/
|-- static/
|   `-- style.css
`-- themes/              # created only when you run `typage theme new ...`
```

The starter includes navigation, a posts collection, a reusable base layout, and a reusable list layout. Search, taxonomies, feeds, and deploy scaffolds remain opt-in so the generated site stays small until you need those features.

## Content Model

Front matter supports page ordering, update metadata, expiration, and typed project-specific data:

```toml
title = "Hello"
description = "A first post"
date = "2026-06-23"
updated = "2026-06-24"
weight = 10
expires = "2027-01-01"
draft = false

[extra]
series = "examples"
math = true
```

By default, future-dated pages and expired pages are not built. Configure this when needed:

```toml
build_future = false
build_expired = false
```

Nested directories become nested sections. For example, `content/posts/tutorials/intro.typ` belongs to section `posts/tutorials`.

The generated site package exposes section data:

```typst
#import "@local/typage:0.1.0": current, section, children, ancestors, siblings

#current.updated
#current.weight
#current.extra

#section("posts/tutorials").pages
#children("posts")
#ancestors(current)
#siblings(current)
```

## Permalinks

By default, Typage uses directory URLs:

```text
content/index.typ             -> /
content/posts/hello.typ       -> /posts/hello/
content/posts/index.typ       -> /posts/
```

You can opt into a global permalink policy in `config.toml`:

```toml
permalink = "/:section/:slug/"
```

Supported placeholders:

```text
:section   page section, for example posts/tutorials
:slug      page slug from front matter or filename
:path      source path without extension, for example posts/hello
:filename  file stem, for example hello
:year      YYYY from date
:month     MM from date
:day       DD from date
```

A page can override the global policy:

```toml
---
title = "Release Note"
date = "2026-06-24"
slug = "v0-23"
permalink = "/releases/:year/:slug/"
aliases = ["old/release-note"]
---
```

Generated page objects expose stable routing fields:

```typst
#current.kind
#current.slug
#current.path
#current.file_path
#current.url
#current.canonical_url
#current.aliases
```

Alias pages are generated as redirects to `canonical_url`.

## Typst-Native Components

Components are theme-owned. The SSG core only exposes data, routing, URL helpers, and the active theme component package.

Use the core package for site data:

```typst
#import "@local/typage:0.1.0": site, current, section, taxonomy-url
```

Use the active theme package for UI components:

```typst
#import "@local/typage-theme:0.1.0": note, callout, card, fig, youtube, page-link, taxonomy-link

#note(title: "Note")[
  This component comes from the active theme.
]

#callout(kind: "warning", title: "Warning")[
  This becomes theme-defined HTML/PDF output.
]

#card(title: "Card", href: "/posts/hello/")[
  Linkable card content.
]

#fig(src: "/images/demo.png", alt: "Demo", caption: "A demo image")

#youtube("dQw4w9WgXcQ", title: "Demo video")
```

The old shortcode syntax remains available as an optional convenience layer:

```typst
{{ note(text="A short note") }}
{{ figure(src="images/demo.png", alt="Demo", caption="A demo figure") }}
{{ youtube(id="dQw4w9WgXcQ", title="Demo video") }}
```

## Theme Management

Themes live under `themes/<name>/`. A theme can provide `templates/`, `templates/components/`, `static/`, and `theme.toml`.

```sh
typage theme new my-theme
typage theme list
typage theme list --verbose
typage theme info my-theme
typage theme check my-theme
```

Enable a theme:

```toml
theme = "my-theme"
```

`theme.toml` schema:

```toml
name = "my-theme"
version = "0.1.0"
description = "A typage theme."
min_typage = "0.1.0"

[components]
note = true
callout = true
card = true
fig = true
youtube = true
page_link = true
taxonomy_link = true

[extra]
accent = "#2563eb"
```

`theme check` validates that:

```text
- theme.toml exists and parses
- min_typage is compatible with the current typage version
- templates/ exists
- templates/components/lib.typ exists
- components declared as true appear to be exported by lib.typ
```

Component convention:

```text
templates/components/
|-- lib.typ       # entrypoint for @local/typage-theme:0.1.0
|-- callout.typ
|-- card.typ
|-- media.typ
`-- layout.typ
```

`typage` copies the active `templates/components/` directory into the generated package `@local/typage-theme:0.1.0`. Project `static/` overrides theme `static/` when both provide the same path.

## Site API

`@local/typage:0.1.0` exposes data and non-presentational helpers for content and templates:

```typst
#import "@local/typage:0.1.0": site, current, pages, sections, taxonomies, url, asset, section, children, ancestors, siblings, taxonomy-url, is-current, page-by-url
#import "@local/typage-theme:0.1.0": page-link, taxonomy-link

= #current.title

#for page in section("posts").pages [
  - #page-link(page)
]

Tags: #taxonomy-link("tags", "typst")
```

Core helpers:

```text
url(path)              prepend site.base_url when configured
asset(path)            alias of url(path)
section(name)          return section data
children(section)      child sections for a section name or section dictionary
ancestors(item)        ancestor sections for a section name, section dictionary, or page
siblings(page)         pages in the same section except the given page
page-by-url(url)       find a page by URL or none
taxonomy-url(name, term)
is-current(page)
```

Presentational helpers such as `page-link`, `taxonomy-link`, `note`, `card`, and media embeds belong to `@local/typage-theme:0.1.0`.

## Directory Policy

Default layout:

```text
content/     Typst source pages
templates/   Typst templates
static/      Static files copied as-is
public/      Generated site output
.typage/     Cache, wrappers, and generated local packages
```

`public/` is disposable. Do not edit it manually. Put manually maintained assets in `static/` or the active theme's `static/`.

If you prefer npm/Vite-like naming, use configuration instead of a different init layout:

```toml
static_dir = "public"
out_dir = "dist"
```

`doctor` and `build` reject dangerous layouts such as `out_dir == static_dir`, `out_dir` inside `content_dir`, or `out_dir` containing source/cache directories.

## Publishing Metadata

When feeds are enabled, Typage writes RSS and, by default, Atom feeds:

```text
/feed.xml
/atom.xml
```

Feed output is configurable:

```toml
feed = true
feed_path = "rss.xml"
feed_sections = ["posts", "projects"]
feed_limit = 0
atom_feed = false

[[feeds]]
path = "projects/rss.xml"
title = "Project Feed"
description = "Project updates."
link = "/projects/"
section = "projects"
limit = 0
```

`feed_limit = 0` means unlimited items. `feed_sections` filters the primary RSS and Atom feeds, while each `[[feeds]]` entry can target a single `section` or a `sections` list.

If `base_url` is empty, feed links are emitted as site-root-relative URLs. Set `base_url` before publishing when you need absolute feed item links. Typage can also write a sitemap and robots file:

```text
/sitemap.xml
/robots.txt
```

Default templates emit:

```text
<link rel="canonical" ...>
og:title / og:type / og:url / og:description
twitter:card / twitter:title / twitter:description
article:published_time for dated pages
```

## Search

When search is enabled, Typage emits `public/search_index.json`. The minimal starter keeps search disabled; add a search page and client script when you want site search.

Suggested optional layout:

```text
content/search.typ
static/search.js
```

The old boolean form remains supported:

```toml
search = true
```

For real sites, prefer the table form:

```toml
[search]
enabled = true
mode = "auto"      # auto | latin | cjk | ngram | ngram-only
ngram = 2          # useful for CJK text
include_body = true
include_headings = true
include_tags = true
include_taxonomies = true
compact = false    # true omits full body text from the JSON index
max_tokens = 2048
```

Search entries include:

```text
title / url / description / excerpt
tokens
headings[] with heading-local tokens
fields.title / fields.headings / fields.body / fields.taxonomies
```

## Build Output and Profiling

Default output is compact and colored. Use `NO_COLOR=1` to disable ANSI colors.

```sh
typage build --force --jobs 4 --profile
typage build --force --jobs 4 --verbose
typage build --explain
typage build --quiet
```

`--profile` prints compile wall time, sum of per-job Typst times, and the slowest pages. `--verbose` prints each compiled page. `--explain` prints why each item is rebuilt or skipped and reports unresolved local dependencies.

## Parallel Compile

`jobs = 0` uses available CPU parallelism. You can also override it from the CLI:

```toml
jobs = 0 # auto
```

```sh
typage build --jobs 4
typage run dev -- --jobs 4
typage serve --live-reload --jobs 4
typage build --jobs 1
```

`current` is passed to Typst with `--input typage_current=<json>`, so each compile job has independent page context and does not race on shared state.

## Scripts

`config.toml` can define scripts:

```toml
[scripts]
build = "build"
dev = "serve --live-reload"
preview = "serve"
check = "doctor"
clean = "clean"
```

Run them:

```sh
typage run
typage run dev
typage run build -- --force
typage run check
```

Convenience aliases:

```sh
typage dev      # run dev
typage preview  # run preview
typage check    # doctor
```

## Hooks

Typage can run command hooks around build, serve, and deploy operations. Hooks are configured in `config.toml` and run from the site root.

```toml
[hooks]
pre_build = "echo before build"
post_build = "echo after build"
pre_serve = "echo before serve"
post_serve = "echo after serve"
pre_deploy = "echo before deploy"
post_deploy = "echo after deploy"
```

Hooks may call external commands or `typage ...`. Nested Typage commands launched from hooks do not trigger hooks again, which avoids accidental recursive hooks. Use `--no-hooks` when you want to bypass hooks explicitly.

```sh
typage build --no-hooks
typage serve --no-hooks
typage deploy init vercel --no-hooks
```

## Commands

```sh
typage build
typage build --force
typage build --pdf
typage build --keep-going
typage dev
typage serve --live-reload
typage preview
typage bundle
typage doctor
typage check
typage clean
```

Create a new page:

```sh
typage new posts/my-post --title "My Post" --date 2026-06-23
```

Create a theme skeleton:

```sh
typage theme new my-theme
```

Enable it in `config.toml`:

```toml
theme = "my-theme"
```

## Deployment Scaffolds

Typage does not upload files by itself. It writes files that common hosting platforms expect:

```sh
typage deploy init github-pages --cname example.com
typage deploy init cloudflare-pages
typage deploy init netlify
typage deploy init vercel
```

Check deployment files:

```sh
typage deploy doctor
typage deploy doctor --target github-pages
```

Generated files:

```text
GitHub Pages:
  .github/workflows/deploy.yml
  static/.nojekyll
  static/CNAME        # only when --cname is passed

Cloudflare Pages:
  wrangler.toml

Netlify:
  netlify.toml

Vercel:
  vercel.json
```

Because `public/` is disposable, files that must appear in deployed output are written to `static/` when appropriate. For example, GitHub Pages uses `static/.nojekyll` and `static/CNAME`, which are copied into `public/` during build.

If a scaffold already exists, Typage refuses to overwrite it. Use `--force` when you intentionally want to regenerate it.

The generated GitHub Pages workflow and Vercel scaffold assume `typage` is installable with `cargo install typage --locked`. During local development, adjust that line to `cargo install --path .` or `cargo install --git <repo>` as needed.

## Crates.io Release

Before publishing, verify the crate package from a clean working tree:

```sh
cargo test
cargo package --allow-dirty
cargo publish --dry-run --allow-dirty
```

When the dry run succeeds, publish with:

```sh
cargo publish
```

`Cargo.lock` is included so deployment scaffolds can use `cargo install typage --locked`.

## Notes

`@local/typage:0.1.0` and `@local/typage-theme:0.1.0` are staged under `.typage/packages/local/` and passed to Typst with `--package-path`.

Typst HTML Export and Bundle Export are experimental. Typage is designed with that moving target in mind.
