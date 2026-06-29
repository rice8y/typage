#import "components/layout.typ": site-header, site-footer, pagination, post-list

#let render(site: (:), page: (:), pages: (), taxonomies: (:), body) = context {
  let asset = p => if site.base_url == "" { p } else { site.base_url + p }
  let full-title = if page.title == site.title { site.title } else { page.title + " | " + site.title }

  if target() == "html" {
    html.elem("html", attrs: (lang: page.lang))[
      #html.elem("head")[
        #html.elem("meta", attrs: (charset: "utf-8"))
        #html.elem("meta", attrs: (name: "viewport", content: "width=device-width, initial-scale=1"))
        #html.elem("meta", attrs: (name: "generator", content: "typage 0.24.2"))
        #if site.base_url != "" {
          html.elem("meta", attrs: (name: "typage-base-url", content: site.base_url))
        }
        #if page.description != none {
          html.elem("meta", attrs: (name: "description", content: page.description))
        }
        #html.elem("title")[#full-title]
        #html.elem("link", attrs: (rel: "canonical", href: asset(page.url)))
        #html.elem("meta", attrs: (property: "og:site_name", content: site.title))
        #html.elem("meta", attrs: (property: "og:title", content: full-title))
        #html.elem("meta", attrs: (property: "og:type", content: "website"))
        #html.elem("meta", attrs: (property: "og:url", content: asset(page.url)))
        #if page.description != none {
          html.elem("meta", attrs: (property: "og:description", content: page.description))
          html.elem("meta", attrs: (name: "twitter:description", content: page.description))
        }
        #html.elem("meta", attrs: (name: "twitter:card", content: "summary"))
        #html.elem("meta", attrs: (name: "twitter:title", content: full-title))
        #html.elem("link", attrs: (rel: "stylesheet", href: asset("/style.css")))
      ]
      #html.elem("body")[
        #site-header(site, asset, pages: pages)
        #html.elem("main", attrs: (class: "page"))[
          #html.elem("header", attrs: (class: "page-header"))[
            #html.elem("p", attrs: (class: "eyebrow"))[#page.kind]
            #html.elem("h1")[#page.title]
            #if page.description != none {
              html.elem("p", attrs: (class: "lead"))[#page.description]
            }
          ]
          #post-list(page, asset)
          #pagination(page, asset)
        ]
        #site-footer(site)
      ]
    ]
  } else {
    set document(title: full-title)
    [= #page.title]
    for item in page.items [
      - #item.title (#item.url)
    ]
    if page.total_pages != none and page.total_pages > 1 [
      Page #page.page_number / #page.total_pages
    ]
    body
  }
}
