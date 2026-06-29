#let frame(alt: none, body) = context {
  if target() == "html" {
    html.elem("figure", attrs: (class: "typst-frame"))[
      #html.frame(body)
      #if alt != none {
        html.elem("figcaption", attrs: (class: "visually-hidden"))[#alt]
      }
    ]
  } else {
    body
  }
}

#let ruby(base, reading) = context {
  if target() == "html" {
    html.elem("ruby")[
      #base
      #html.elem("rt")[#reading]
    ]
  } else {
    base
  }
}

#let card(body) = context {
  if target() == "html" {
    html.elem("section", attrs: (class: "card"))[#body]
  } else {
    block(inset: 8pt, stroke: luma(180))[#body]
  }
}
