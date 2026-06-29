---
title = "Typage"
description = "A small Typst-native starter for static sites"
template = "base.typ"
---

#import "@local/typage-theme:0.1.0": callout, card

Typage is an experimental SSG that lets you write content, templates, and theme components in Typst, then publish static HTML.

#callout(title: "Start here")[
  Edit `content/index.typ` to change this page. To add a post, run `typage new posts/my-post --title "My Post"`.
]

#card(title: "Content first", href: "/posts/hello/")[
  Use front matter, section pages, internal links, taxonomies, and search indexes without leaving Typst.
]

#card(title: "Themeable", href: "/about/")[
  Layout lives in `templates/`, styling lives in `static/style.css`, and reusable themes can be created with `typage theme new`.
]

== Next steps

- Edit `content/posts/hello.typ`
- Customize the shared layout in `templates/base.typ`
- Set `base_url` in `config.toml` before publishing
