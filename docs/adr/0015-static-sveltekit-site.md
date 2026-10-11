# 0015. One static SvelteKit site for the rumble and the user documentation

- Status: Accepted
- Date: 2026-10-11

## Context

The rumble needs a website (design §12.8), and robocode-s needs a home for user documentation: what the project is, how to get started, how to develop, debug and release a robot. The site is mostly reading: rankings tables, bot pages, docs. The heavy parts (pairing heat maps, the WebAssembly replay viewer) draw on a canvas and don't depend on a UI framework. The data changes daily; the site itself rarely. Hosting is S3 + CloudFront within the free tier (ADR 0007), so there is no server to render pages.

React was considered: a larger ecosystem and more familiar to contributors. For a static site it needs more assembly (Vite + a router + MDX, or Next.js static export, which handles client-only dynamic routes like `/bot/<id>` less naturally).

## Decision

- **SvelteKit with `adapter-static`** in `web/`, serving both the rumble and the user documentation from `rumble.planethofstede.be`.
- **Docs and the home page are prerendered** HTML. Docs are Markdown in the repository (mdsvex), versioned with the code.
- **Data pages render in the browser** from a `200.html` fallback and read static JSON files written by the rankings Lambda. The files form a versioned contract (`docs/rumble-data.md`, `web/src/lib/rumble/types.ts`).
- Until the backend exists, the dev and preview servers serve **seeded synthetic data** in that contract, so the site and the backend can be built independently.
- Design: retro Robocode (dark battlefield, radar green, pixel display font), self-hosted fonts.

## Consequences

- Project and engine internals stay in `docs/` for contributors; the site's docs are for robot authors.
- Data pages aren't indexed well by search engines (rendered in the browser). Acceptable: the docs, which people search for, are prerendered.
- CloudFront needs a small function for extensionless URLs and an error response mapping to `200.html` (`web/README.md`).
