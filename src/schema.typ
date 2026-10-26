#let optional(inner, default: none) = (kind: "optional", inner: inner, default: default)
#let array(inner: none, ..args) = (kind: "array", inner: if args.pos().len() == 0 { inner } else { args.pos().first() })
#let dictionary(inner: none, ..args) = (kind: "dictionary", inner: if args.pos().len() == 0 { inner } else { args.pos().first() })
#let object(fields) = (kind: "object", fields: fields)
#let union(..options) = {
  assert(options.pos().len() > 0, message: "union requires at least one schema")
  (kind: "union", options: options.pos())
}
#let any = (kind: "any")
#let url = (kind: "url")
#let collection(schema: (:)) = schema
#let string = str
#let boolean = bool
#let integer = int

#let __matches(value, schema) = {
  if type(schema) == type {
    // Date strings remain accepted for existing content.
    type(value) == schema or (schema == datetime and type(value) == str)
  } else if schema == none or schema == auto {
    value == schema
  } else if type(schema) == function {
    schema(value) == true
  } else if type(schema) == std.dictionary {
    let kind = schema.at("kind", default: "object")
    if kind == "any" { true }
    else if kind == "optional" { value == none or __matches(value, schema.inner) }
    else if kind == "array" {
      type(value) == std.array and (schema.inner == none or value.all(v => __matches(v, schema.inner)))
    } else if kind == "dictionary" {
      type(value) == std.dictionary and (schema.inner == none or value.values().all(v => __matches(v, schema.inner)))
    } else if kind == "object" {
      let fields = schema.at("fields", default: schema)
      type(value) == std.dictionary and fields.pairs().all(((name, inner)) => __matches(value.at(name, default: none), inner))
    } else if kind == "union" { schema.options.any(inner => __matches(value, inner)) }
    else if kind == "url" {
      type(value) == str and value.contains(regex("^https?://[^\\s/]+(?:/[^\\s]*)?$"))
    } else { panic("unknown schema kind: " + kind) }
  } else { panic("schema must be a Typst type, validator, none, auto, or schema helper") }
}

#let __validate(candidate, schema, source: "", core: ()) = {
  for name in candidate.keys() {
    assert(name in schema or name in core, message: source + ": metadata field `" + name + "` is not declared in collection schema")
  }
  for (name, field) in schema {
    let is-optional = type(field) == std.dictionary and field.at("kind", default: "") == "optional"
    if name not in candidate and is-optional and field.default != none {
      candidate.insert(name, field.default)
    }
    assert(name in candidate or is-optional, message: source + ": missing required metadata field `" + name + "`")
    if name in candidate {
      assert(__matches(candidate.at(name), field), message: source + ": metadata field `" + name + "` does not match its schema (received " + repr(type(candidate.at(name))) + ")")
    }
  }
  candidate
}
