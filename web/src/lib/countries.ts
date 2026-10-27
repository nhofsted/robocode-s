// The countries the account page offers: those with a flag in flag-icons (Flag.svelte), by
// English name. Regions that are not countries (eu, un) are left out.
const FILES = import.meta.glob<string>('/node_modules/flag-icons/flags/4x3/*.svg', {
	query: '?url',
	import: 'default'
});

const names = new Intl.DisplayNames(['en'], { type: 'region' });

export const COUNTRIES: { code: string; name: string }[] = Object.keys(FILES)
	.map((path) => /\/([a-z]{2})\.svg$/.exec(path)?.[1])
	.filter((code): code is string => !!code && code !== 'eu' && code !== 'un')
	.map((code) => ({ code, name: names.of(code.toUpperCase()) ?? code.toUpperCase() }))
	.sort((a, b) => a.name.localeCompare(b.name, 'en'));

// The RoboWiki's own flags (RoboRumble/Country_Flags), offered after the countries: `WIKI`
// (the United Nations flag) and `MAN`.
export const WIKI_FLAGS: { code: string; name: string }[] = [
	{ code: 'un', name: 'United Nations (wiki bots)' },
	{ code: 'ancap', name: 'Anarcho-capitalist' }
];
