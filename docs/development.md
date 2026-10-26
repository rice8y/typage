# Development

## Code Style

Rust follows the [Rust Style Guide](https://doc.rust-lang.org/style-guide/)
using rustfmt's default settings. Typst sources use
[Typstyle](https://typstyle-rs.github.io/typstyle/) with its default settings.

```sh
cargo install typstyle --version 0.15.2 --locked
cargo fmt --all
typstyle -i src/schema.typ src/runtime.typ templates content
```

Before submitting changes, run:

```sh
cargo fmt --all --check
typstyle --check src/schema.typ src/runtime.typ templates content
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Watch integration tests require an environment that delivers filesystem events.

## Build Responsibilities

- `src/build.rs` coordinates builds, compilation, caching, and publication.
- `src/build/plan.rs` discovers and validates the shared build plan.
- `src/build/outputs.rs` owns output paths, collisions, redirects, and cleanup.
- `src/build/print.rs` selects combined PDF content and prepares print wrappers.
- `src/build/pagination.rs` constructs section and taxonomy list pages.
- `src/build/feeds.rs` serializes RSS, Atom, sitemap, and robots outputs.
- `src/build/search.rs` extracts text, tokenizes content, and builds the search index.
- `src/build/theme.rs` scaffolds and inspects themes and resolves active theme paths.

Domain-specific unit tests live with their implementations. Cross-cutting build
tests live in `src/build/tests.rs`; CLI integration tests live in `tests/`.

`src/metadata.rs` bridges Rust and Typst metadata evaluation. Schema constructors,
collection normalization, and validation live in `src/schema.typ`. The generated
`@local/typage` package's runtime helpers live in `src/runtime.typ` and are embedded
with `include_str!`, so they can be formatted independently of Rust source.
