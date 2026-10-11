// The docs sidebar, in reading order. Pages live in src/routes/docs/<slug>/+page.md.
export const DOCS = [
	{ href: '/docs', title: 'Overview' },
	{ href: '/docs/about', title: 'About robocode-s' },
	{ href: '/docs/getting-started', title: 'Getting started' },
	{ href: '/docs/robot-development', title: 'Developing a robot' },
	{ href: '/docs/rumble', title: 'The rumble' },
	{ href: '/docs/community', title: 'Community and links' }
] as const;
