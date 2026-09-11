# Gateway website

A responsive landing page and searchable documentation for gRPC–GraphQL Gateway,
inspired by the Ruby UTCP website. The site uses a dark palette, cyan and magenta
accents, the supplied gateway logo, and examples taken from the gateway docs.

## Run locally

Requires Node.js 22 or newer:

```sh
npm ci
npm run dev
```

Open http://127.0.0.1:4173. To choose another port, use
`npm run dev -- --port 4175`. The server only exposes `website/dist`, binds to
loopback, and does not run the Rust gateway. After editing, run `npm run build`
and refresh the browser.

## Build and check

```sh
npm run check
npm run build
npm run preview
```

Deploy the contents of `website/dist/` to a static host. The existing GitHub Pages
workflow builds and checks the website on pull requests, and publishes on pushes
to `main` or manual runs on `main`. The published site is
[protocol-lattice.github.io/grpc_graphql_gateway](https://protocol-lattice.github.io/grpc_graphql_gateway/).
The website needs no backend or runtime packages.

`npm run dev` is for local preview. GitHub Actions runs `npm ci --ignore-scripts`
and `npm run check` (which builds the site), then deploys `website/dist/` with the
official GitHub Pages actions. It does not need a persistent Node.js server.
The repository's **Settings → Pages → Build and deployment → Source** must be
**GitHub Actions**. To publish manually, open **Actions → Website and
Documentation → Run workflow** and choose `main`.

All internal URLs are relative. Test project-path hosting locally with:

```sh
npm run preview -- --base /grpc_graphql_gateway/ --port 4175
```

The landing page is `index.html`; the documentation entry is `introduction.html`.
Existing guide URLs such as `getting-started/quick-start.html` are preserved.
The original mdBook configuration is also available for a separate mdBook build.

## Content and maintenance

- `public/index.html`: landing-page template and service diagram.
- `public/styles.css`: shared responsive theme and reduced-motion support.
- `public/app.js`: keyboard-accessible tabs, example responses, clipboard
  controls, mobile navigation, and local documentation search.
- `public/assets/gateway-logo.svg`: a self-contained SVG embedding the original
  supplied PNG unchanged. CSS crops the surrounding whitespace; a filter inside
  the SVG makes its white background transparent against the dark theme.
- `examples.mjs`: annotated proto, GraphQL, and Rust examples.
- `components.mjs`: shared header, footer, search dialog, and icons.
- `build.mjs`: renders all `docs/src/**/*.md` with Marked and Highlight.js.
  Navigation follows `docs/src/SUMMARY.md`; version labels come from `Cargo.toml`.
- `check.mjs`: checks page headings, IDs, relative links, anchors, assets,
  accessible control targets, and search-index completeness.
- `serve.mjs`: local static preview server, restricted to the generated output.

Edit the Markdown in `docs/src/` to update a guide. Documentation content is
rendered at build time and remains readable without JavaScript. Search opens with
Command/Ctrl+K and supports arrow keys, Enter, and Escape. Code examples are
illustrations; the example-response controls do not send requests or mutate data.
The Rust quick start requires `graphql.proto`, a compiled descriptor set, and a
running matching gRPC service, as explained in the linked setup guides.

DM Sans and IBM Plex Mono load from Google Fonts, with local system fallbacks.
All other website assets and the search index are served from the build output.
