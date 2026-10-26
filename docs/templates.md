# Templates and Themes

Theme components and the site data API available to Typst content and templates.

## Typst-Native Components

Components are theme-owned. The SSG core only exposes data, routing, URL helpers, and the active theme component package.

Use the core package for site data:

```typst
#import "@local/typage:0.2.1": site, current, section, taxonomy-url
```

Use the active theme package for UI components:

```typst
#import "@local/typage-theme:0.2.1": note, callout, card, fig, youtube, page-link, taxonomy-link

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
min_typage = "0.2.0"

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
|-- lib.typ       # entrypoint for @local/typage-theme:0.2.1
|-- callout.typ
|-- card.typ
|-- media.typ
`-- layout.typ
```

`typage` copies the active `templates/components/` directory into the generated package `@local/typage-theme:0.2.1`. Project `static/` overrides theme `static/` when both provide the same path.

## Site API

`@local/typage:0.2.1` exposes data and non-presentational helpers for content and templates:

```typst
#import "@local/typage:0.2.1": site, current, pages, sections, taxonomies, url, asset, section, children, ancestors, siblings, taxonomy-url, is-current, page-by-url
#import "@local/typage-theme:0.2.1": page-link, taxonomy-link

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

Presentational helpers such as `page-link`, `taxonomy-link`, `note`, `card`, and media embeds belong to `@local/typage-theme:0.2.1`.
