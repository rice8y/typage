#import "components/layout.typ": site-header, site-footer, page-nav, toc

#let render(site: (:), page: (:), pages: (), taxonomies: (:), body) = context {
  let asset = p => if site.base_url == "" { p } else { site.base_url + p }
  let is-home = page.url == "/"
  let page-title = if page.title == none { site.title } else { page.title }
  let full-title = if page.title == none or page.title == site.title { site.title } else { page.title + " | " + site.title }
  let og-type = if page.kind == "page" and page.date != none { "article" } else { "website" }
  let page-label = if is-home { "home" } else if page.section == "pages" { "page" } else { page.section }

  if target() == "html" {
    html.elem("html", attrs: (lang: page.lang))[
      #html.elem("head")[
        #html.elem("meta", attrs: (charset: "utf-8"))
        #html.elem("meta", attrs: (name: "viewport", content: "width=device-width, initial-scale=1"))
        #html.elem("meta", attrs: (name: "generator", content: "typage 0.1.2"))
        #if site.base_url != "" {
          html.elem("meta", attrs: (name: "typage-base-url", content: site.base_url))
        }
        #if page.description != none {
          html.elem("meta", attrs: (name: "description", content: page.description))
        }
        #html.elem("title")[#full-title]
        #if page.url != none {
          html.elem("link", attrs: (rel: "canonical", href: asset(page.url)))
        }
        #html.elem("meta", attrs: (property: "og:site_name", content: site.title))
        #html.elem("meta", attrs: (property: "og:title", content: full-title))
        #html.elem("meta", attrs: (property: "og:type", content: og-type))
        #if page.url != none {
          html.elem("meta", attrs: (property: "og:url", content: asset(page.url)))
        }
        #if page.description != none {
          html.elem("meta", attrs: (property: "og:description", content: page.description))
          html.elem("meta", attrs: (name: "twitter:description", content: page.description))
        }
        #if page.date != none and page.kind == "page" {
          html.elem("meta", attrs: (property: "article:published_time", content: page.date))
        }
        #html.elem("meta", attrs: (name: "twitter:card", content: "summary"))
        #html.elem("meta", attrs: (name: "twitter:title", content: full-title))
        #html.elem("link", attrs: (rel: "stylesheet", href: asset("/style.css")))
      ]
      #html.elem("body")[
        #site-header(site, asset, pages: pages)
        #html.elem("main", attrs: (class: if is-home { "page page-home" } else { "page" }))[
          #html.elem("header", attrs: (class: "page-header"))[
            #html.elem("p", attrs: (class: "eyebrow"))[#if page.kind == "page" { page-label } else { page.kind }]
            #html.elem("h1")[#page-title]
            #if page.description != none {
              html.elem("p", attrs: (class: "lead"))[#page.description]
            }
            #if page.date != none {
              html.elem("p", attrs: (class: "date"))[#page.date]
            }
          ]
          #toc(page)
          #html.elem("article", attrs: (class: "content"))[#body]
          #page-nav(page, asset)
        ]
        #site-footer(site)
      ]
    ]
  } else {
    set document(title: full-title)
    [= #page-title]
    body
  }
}
