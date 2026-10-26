// Embedded as the generated @local/typage package's lib.typ, not imported from src.
// data.typ contains the site graph; the compiler injects __typage_values separately.
#import "data.typ": generated, pages, sections, site, taxonomies

// Large current-page payloads use a file; smaller payloads travel in sys.inputs.
#let __typage_current_file = sys.inputs.at("typage_current_file", default: none)
#let __typage_current_raw = if __typage_current_file == none {
  sys.inputs.at("typage_current", default: "{}")
} else { read(__typage_current_file) }
// JSON carries registry markers for native values such as content and functions.
// Restore them recursively so current and site-page metadata expose the same types.
#let __typage_hydrate(value) = {
  if type(value) == dictionary {
    if "__typage_native" in value {
      __typage_values.at(value.__typage_native)
    } else {
      value
        .pairs()
        .fold((:), (result, (key, item)) => {
          result.insert(key, __typage_hydrate(item))
          result
        })
    }
  } else if type(value) == array { value.map(__typage_hydrate) } else { value }
}
#let current = __typage_hydrate(json(bytes(__typage_current_raw)))

#let url(path) = if site.base_url == "" { path } else { site.base_url + path }
#let asset = url

#let is-current(page) = page.url == current.url

#let page-by-url(target) = {
  let found = pages.filter(page => page.url == target)
  if found.len() == 0 { none } else { found.first() }
}

#let section(name) = {
  let found = sections.filter(sec => sec.name == name)
  // Keep the section shape predictable even when no matching section was built.
  if found.len() == 0 {
    (
      name: name,
      title: name,
      description: none,
      path: "/" + name + "/",
      parent: none,
      ancestors: (),
      children: (),
      pages: (),
      sort_by: none,
      weight: none,
    )
  } else { found.first() }
}

#let children(item) = {
  let name = if type(item) == str { item } else { item.name }
  section(name).children.map(child => section(child))
}

#let ancestors(item) = {
  let name = if type(item) == str { item } else if (
    item.at("section", default: none) != none
  ) { item.section } else { item.name }
  section(name).ancestors.map(parent => section(parent))
}

#let siblings(page) = pages.filter(other => (
  other.section == page.section and other.url != page.url
))

#let taxonomy-url(name, term) = {
  let tax = taxonomies.at(name, default: (slug: name))
  "/" + tax.slug + "/" + str(term).replace(" ", "-") + "/"
}
