// Print template for combined PDF documents.
// Typage calls `render` with site data, document metadata, selected page metadata,
// and a generated body that contains the selected pages in order.

#let render(site: (:), document: (:), pages: (), body) = {
  set page(numbering: "1")

  align(center)[
    #text(size: 24pt, weight: "bold")[#document.title]

    #if document.description != none [
      #v(1em)
      #text(size: 11pt)[#document.description]
    ]

    #v(1em)
    #text(size: 9pt)[#site.title]
  ]

  pagebreak()
  outline()
  pagebreak()

  body
}
