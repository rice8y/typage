#let card(body, title: none, href: none) = context {
  if target() == "html" {
    if href == none {
      html.elem("section", attrs: (class: "card"))[
        #if title != none { html.elem("h2", attrs: (class: "card-title"))[#title] }
        #body
      ]
    } else {
      html.elem("section", attrs: (class: "card card-link"))[
        #if title != none {
          html.elem("h2", attrs: (class: "card-title"))[
            #html.elem("a", attrs: (href: href))[#title]
          ]
        }
        #body
      ]
    }
  } else {
    block(inset: 8pt, stroke: luma(180), radius: 4pt)[
      #if title != none { strong(title) }
      #body
    ]
  }
}
