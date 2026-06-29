---
title = "Hello Typage"
description = "Your first article with Typst HTML export"
date = "2026-06-28"
template = "base.typ"
tags = ["typst", "ssg"]
---

Typage lets you write both article content and site templates in Typst.

== Write in Typst

Use normal prose, headings, lists, code blocks, and links.

```typst
= Hello

Generate HTML from Typst.
```

== Link pages

Internal links such as `@/posts/hello.typ` are resolved to public URLs at build time.

Home: #link("@/index.typ")[Back to the home page]
