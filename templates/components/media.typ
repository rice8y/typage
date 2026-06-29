#import "@local/typage:0.1.0": url

#let fig(body: none, src: none, alt: "", caption: none) = context {
  if target() == "html" {
    html.elem("figure", attrs: (class: "figure"))[
      #if src != none { html.elem("img", attrs: (src: url(src), alt: alt))[] }
      #if body != none { body }
      #if caption != none { html.elem("figcaption")[#caption] }
    ]
  } else {
    if src != none {
      if caption != none { figure(image(src), caption: caption) } else { image(src) }
    } else if body != none {
      if caption != none { figure(body, caption: caption) } else { body }
    }
  }
}

#let youtube(id, title: "YouTube video") = context {
  let src = "https://www.youtube-nocookie.com/embed/" + id
  if target() == "html" {
    html.elem("div", attrs: (class: "embed embed-youtube"))[
      #html.elem("iframe", attrs: (src: src, title: title, loading: "lazy", allowfullscreen: ""))[]
    ]
  } else {
    link(src)[#title]
  }
}
