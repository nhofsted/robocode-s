// Pages are prerendered (docs, home) unless they opt out; the data pages under /rankings,
// /bot and /pairing render in the browser from the 200.html fallback (vite.config.ts).
export const prerender = true;
export const trailingSlash = 'never';
