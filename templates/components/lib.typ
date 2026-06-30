#import "@local/typage:0.1.3": taxonomy-url
#import "callout.typ": note, callout
#import "card.typ": card
#import "media.typ": fig, youtube

#let page-link(page, body: none) = {
  let label = if body == none { page.title } else { body }
  link(page.url)[#label]
}

#let taxonomy-link(name, term, body: none) = {
  let label = if body == none { str(term) } else { body }
  link(taxonomy-url(name, term))[#label]
}
