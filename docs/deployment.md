# Deployment

Generate configuration files for supported hosting platforms.

## Deployment Scaffolds

Typage does not upload files by itself. It writes files that common hosting platforms expect:

```sh
typage deploy init github-pages --cname example.com
typage deploy init cloudflare-pages
typage deploy init netlify
typage deploy init vercel
```

Check deployment files:

```sh
typage deploy doctor
typage deploy doctor --target github-pages
```

Generated files:

```text
GitHub Pages:
  .github/workflows/deploy.yml
  static/.nojekyll
  static/CNAME        # only when --cname is passed

Cloudflare Pages:
  wrangler.toml

Netlify:
  netlify.toml

Vercel:
  vercel.json
```

Because `public/` is disposable, files that must appear in deployed output are written to `static/` when appropriate. For example, GitHub Pages uses `static/.nojekyll` and `static/CNAME`, which are copied into `public/` during build.

If a scaffold already exists, Typage refuses to overwrite it. Use `--force` when you intentionally want to regenerate it.

Generated deploy scaffolds install Typage with `cargo install typage --locked` before running `typage build`. Typage includes the compiler, so a separate Typst installation is unnecessary. Regenerate existing scaffolds with `typage deploy init <target> --force` to remove the old Typst CLI installation steps.

During local development, adjust `cargo install typage --locked` to `cargo install --path .` or `cargo install --git https://github.com/rice8y/typage` as needed.
