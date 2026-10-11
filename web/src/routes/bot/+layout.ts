// Rumble data changes daily and the site is deployed rarely, so the data pages are not
// prerendered: they load the JSON files in the browser (lib/rumble/data.ts).
export const prerender = false;
export const ssr = false;
