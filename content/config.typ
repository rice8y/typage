// Typage reads this file as collection schema metadata.
// The schema identifiers are Typage built-ins and do not need imports.
#let collections = (
  pages: collection.with(schema: (
    title: str,
    description: optional(str),
    template: optional(str),
    weight: optional(int),
  )),
  posts: collection.with(schema: (
    title: str,
    description: optional(str),
    date: optional(datetime),
    updated: optional(datetime),
    draft: optional(bool),
    template: optional(str),
    tags: optional(array(str)),
  )),
)
