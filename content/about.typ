#show: page.with(
  title: "About",
  description: "How this starter is organized",
  template: "base.typ",
  weight: 20,
)

The Typage starter separates core behavior, templates, content, and static assets so each part can grow independently.

== Project shape

- `content/`: pages written in Typst
- `templates/`: Typst templates shared by HTML and PDF output
- `templates/components/`: theme-owned UI components
- `static/`: assets copied directly into the generated site

== Production

Before publishing, set `base_url` in `config.toml` if you need absolute URLs for canonical links, Open Graph metadata, feeds, or sitemaps.
