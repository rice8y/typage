# Building and PDF Export

Compiler configuration, output directories, PDF export, and build commands.

## Embedded Compiler

HTML, per-page PDF, combined print PDF, and bundle exports all run in-process. `typage doctor` reports the embedded compiler version. The old `--typst` option has been removed; remove `typst = "..."` from existing configuration files. The compiler version is selected by Typage's dependencies rather than an executable on `PATH`.

The embedded compiler is Typst 0.15.1. HTML builds enable `features = "html"` by default. `typage bundle` compiles the site as a Typst bundle with `bundle_features = "bundle,html"`; both exporters remain experimental.

System fonts and Typst's standard embedded fonts are available. Additional fonts can be supplied through `TYPST_FONT_PATHS` (an OS-separated path list). Set `TYPST_IGNORE_SYSTEM_FONTS=true` or `TYPST_IGNORE_EMBEDDED_FONTS=true` to disable the corresponding source. Generated `@local/typage` and `@local/typage-theme` packages take precedence over system packages. Other packages are loaded from the standard Typst data/cache directories or downloaded from Typst Universe when missing. Downloads require network access on the first build.

`TYPST_PACKAGE_PATH`, `TYPST_PACKAGE_CACHE_PATH`, and `TYPST_CERT` customize package locations and the HTTPS certificate. Relative paths are resolved from the project root. `SOURCE_DATE_EPOCH` fixes Typst's current date and PDF creation timestamp for reproducible builds. Experimental features are configured with `features` and `bundle_features` in `config.toml`.

Each compile job has its own sources and inputs; fonts and package loading are shared within a build. File reads are restricted to the project or the imported package's root, including symlink checks. The cache records actual Typst source and file reads, including dynamic paths and imported packages, and hashes binary contents without text conversion. Changed or deleted dependencies invalidate cached outputs; `--explain` identifies them. Compiler upgrades also invalidate the cache. After changing installed fonts, use `typage build --force`.

## Watch and Live Reload

`typage watch` and `typage serve --live-reload` share a build session. Saves are debounced after the last change, and changes arriving during compilation remain queued. The session watches configuration files, plugin configuration, source directories (including newly created directories), and recorded compiler dependencies. Configuration changes take effect on the next build, including a changed output directory in live preview.

Fonts and package loaders are reused while compiler configuration is unchanged; sources are loaded afresh per compile. Both modes run `pre_build` and `post_build` hooks unless `--no-hooks` is supplied. A failed rebuild does not advance the live-reload revision.

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

All public output paths are validated before compilation. Content, aliases, generated lists, enabled PDFs, feeds, search, sitemap, robots, and static assets must not collide, including file/directory conflicts. Project static files may override theme static files at the same path.

Compiled outputs and generated assets are staged before publication. A compilation or asset-generation failure leaves published files and the build cache unchanged, including with `--keep-going`. Publication uses atomic replacement of individual files and then stale-file cleanup; it is not an atomic directory swap. An I/O failure during publication can still leave a mixture of old and new files. Parent-directory symlinks are rejected for writes; these checks are not a sandbox against another process concurrently replacing directories.

## Build Output and Profiling

Default output is compact and colored. Use `NO_COLOR=1` to disable ANSI colors.

```sh
typage build --force --jobs 4 --profile
typage build --force --jobs 4 --verbose
typage build --explain
typage build --quiet
```

`--profile` prints compile wall time, sum of per-job Typst times, and the slowest pages. `--verbose` prints each compiled page. `--explain` prints why each item is rebuilt or skipped and reports unresolved local dependencies.

## Print PDF / Combined PDF Export

`typage build --pdf` writes one `index.pdf` next to each built content page.

Use `[[pdf_documents]]` when you want one PDF that combines multiple content pages into a print document:

```toml
[[pdf_documents]]
path = "print.pdf"
title = "r-Portfolio"
description = "Print version of r-Portfolio."
template = "print.typ"
sections = ["blog", "projects"]
sort_by = "date_desc"
section_headings = ["blog", "projects"]
section_heading_level = 1
include_drafts = false
include_future = false
include_expired = false

[[pdf_documents]]
path = "projects.pdf"
title = "Projects"
template = "print.typ"
pages = ["projects/typage.typ", "/projects/typshade/"]
```

`path` is a PDF path relative to `out_dir`. `template` is resolved relative to `templates_dir`. `sections` selects content pages by section, while `pages` can explicitly list source paths, Typage content paths, or page URLs. Explicit `pages` keep their listed order; section-selected pages use the same `sort_by` vocabulary as section lists.

Set `section_headings` to insert section-level headings before pages from those sections. Those generated section headings use `section_heading_level`, and page titles in those sections are emitted one level deeper. For example, `section_headings = ["blog", "projects"]` makes `Blog` and `Projects` level-1 headings while individual post and project titles become level-2 headings.

Combined print PDFs are generated during `typage build`, served like any other file from `public/`, and participate in the incremental cache, `--force`, `--explain`, stale output cleanup, and `doctor` validation.

A minimal `templates/print.typ`:

```typst
#let render(site: (:), document: (:), pages: (), body) = {
  set page(numbering: "1")

  align(center)[
    #text(size: 24pt, weight: "bold")[#document.title]
  ]

  pagebreak()
  outline()
  pagebreak()

  body
}
```

Typage calls `render` with `site`, `document`, selected page metadata, and `body`. The generated `body` contains the selected content pages in order, with a local `page` variable available for each page block. Typage sets the PDF document title before calling `render`; inside the template, use `document.title` for visible text. HTML-only content or templates that call `html.elem(...)` must guard those calls with `target() == "html"` or provide a PDF fallback.

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

`current` is passed through the job's `sys.inputs.typage_current` JSON value (or a generated metadata file for large values), so each compile job has independent page context. Fonts and package resolution are shared across jobs.

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
