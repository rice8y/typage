#import "@local/typage:0.2.1": taxonomy-url
#import "callout.typ": callout, note
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
