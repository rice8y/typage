#let callout(body, kind: "note", title: none) = context {
  let classes = "callout callout-" + str(kind)
  if target() == "html" {
    html.elem("aside", attrs: (class: classes))[
      #if title != none { html.elem("p", attrs: (class: "callout-title"))[#title] }
      #body
    ]
  } else {
    block(inset: 8pt, stroke: luma(180), radius: 4pt)[
      #if title != none { strong(title) }
      #body
    ]
  }
}

#let note(body, title: none) = callout(body, kind: "note", title: title)
