# web: the robocode-s website

The rumble website and the project's user documentation, served at `rumble.planethofstede.be` (design §12.1, §12.8; ADR 0015). SvelteKit with `adapter-static`: a static site on S3 + CloudFront, no server.

```sh
pnpm install
pnpm dev          # http://localhost:5173, with generated sample data
pnpm check        # type check (svelte-check)
pnpm test         # unit tests (vitest)
pnpm build        # static site in build/
pnpm preview      # serve build/, with generated sample data
```

Requires Node 24 and pnpm.

## Layout

- `src/routes/+page.svelte`: home page.
- `src/routes/docs/**/+page.md`: documentation for robot authors, in Markdown (mdsvex). Frontmatter `title` and `description` are required; the sidebar is `src/lib/docs/nav.ts`. Blockquotes starting with **Status:** or **Tip:** render as callouts. Curly braces in Markdown text are Svelte expressions: put them in code.
- `src/routes/rankings`, `bot/[id]`, `pairing/[rumble]/[a]/[b]`: the data pages. They are not prerendered; they fetch the rumble's JSON files in the browser.
- `src/lib/rumble/types.ts`: **the data contract** with the rankings Lambda (`docs/rumble-data.md`).
- `src/lib/rumble/fixtures.ts`: seeded synthetic data in that contract. `vite dev` and `vite preview` serve it at `/data/v1`; builds never contain it.

Library imports use `#lib/...` with the file extension (SvelteKit 3).

## Configuration

| Variable | Effect |
|---|---|
| `VITE_RUMBLE_DATA` | Base URL of the data files (default `/data/v1`). |
| `VITE_RUMBLE_LIVE=true` | Hides the "sample data" notice once the rumble runs for real. |
| `RUMBLE_FIXTURES=off` | Don't serve sample data in dev/preview. |

## Hosting

`build/` is uploaded to the site bucket; the rankings Lambda writes `data/v1/` next to it. CloudFront needs:

- **Pretty URLs:** prerendered pages are `docs/about.html` etc. A CloudFront Function maps an extensionless path to `<path>.html` (and `/` to `/index.html`).
- **SPA fallback:** 403/404 from S3 → `/200.html` with status 200, so `/bot/<id>` and other data pages render in the browser. Data requests under `/data/` must keep their 404.
- **Caching:** `_app/immutable/*` forever; HTML short (minutes); `data/*` until shortly after the next daily rankings run.
