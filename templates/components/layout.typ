#let top-nav(site, asset, pages: ()) = {
  let top-pages = pages.filter(page => page.section == "pages" and page.url != "/")
  html.elem("nav", attrs: (class: "site-nav", aria-label: "Primary navigation"))[
    #html.elem("a", attrs: (href: asset("/posts/")))[Posts]
    #for page in top-pages {
      html.elem("a", attrs: (href: asset(page.url)))[#page.title]
    }
  ]
}

#let site-header(site, asset, pages: ()) = html.elem("header", attrs: (class: "site-header"))[
  #html.elem("div", attrs: (class: "site-header-inner"))[
    #html.elem("a", attrs: (href: asset("/"), class: "brand"))[#site.title]
    #top-nav(site, asset, pages: pages)
  ]
]

#let site-footer(site) = html.elem("footer", attrs: (class: "site-footer"))[
  #html.elem("p")[Built with Typage and Typst HTML Export.]
  #if site.base_url == "" {
    html.elem("p", attrs: (class: "muted"))[Set `base_url` in `config.toml` before publishing.]
  }
]

#let page-nav(page, asset) = if page.prev != none or page.next != none {
  html.elem("nav", attrs: (class: "page-nav", aria-label: "Page navigation"))[
    #if page.prev != none {
      html.elem("a", attrs: (href: asset(page.prev.url), rel: "prev"))[Previous: #page.prev.title]
    }
    #if page.next != none {
      html.elem("a", attrs: (href: asset(page.next.url), rel: "next"))[Next: #page.next.title]
    }
  ]
} else { none }

#let pagination(page, asset) = if page.total_pages != none and page.total_pages > 1 {
  html.elem("nav", attrs: (class: "page-nav", aria-label: "Pagination"))[
    #if page.prev != none {
      html.elem("a", attrs: (href: asset(page.prev.url), rel: "prev"))[Previous]
    }
    #html.elem("span")[Page #str(page.page_number) / #str(page.total_pages)]
    #if page.next != none {
      html.elem("a", attrs: (href: asset(page.next.url), rel: "next"))[Next]
    }
  ]
} else { none }

#let toc(page) = if page.toc.len() > 0 {
  html.elem("nav", attrs: (class: "toc", aria-label: "Table of contents"))[
    #html.elem("strong")[Contents]
    #html.elem("ol")[
      #for item in page.toc {
        html.elem("li", attrs: (class: "toc-level-" + str(item.level)))[
          #html.elem("a", attrs: (href: "#" + item.id))[#item.text]
        ]
      }
    ]
  ]
} else { none }

#let post-list(page, asset) = html.elem("ol", attrs: (class: "post-list"))[
  #for item in page.items {
    html.elem("li")[
      #html.elem("article", attrs: (class: "post-list-item"))[
        #html.elem("h2")[
          #html.elem("a", attrs: (href: asset(item.url)))[#item.title]
        ]
        #if item.date != none { html.elem("p", attrs: (class: "date"))[#item.date] }
        #if item.excerpt != none { html.elem("p")[#item.excerpt] } else if item.description != none { html.elem("p")[#item.description] }
        #if item.tags.len() > 0 {
          html.elem("p", attrs: (class: "tags"))[
            #for tag in item.tags {
              html.elem("span")[#tag]
            }
          ]
        }
      ]
    ]
  }
]
