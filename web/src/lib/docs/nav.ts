// The docs sidebar, in reading order. Pages live in src/routes/docs/<slug>/+page.md.
export type DocLink = { href: string; title: string; children?: readonly DocLink[] };

export const DOCS: readonly DocLink[] = [
	{ href: '/docs', title: 'Overview' },
	{ href: '/docs/getting-started', title: 'Getting started' },
	{
		href: '/docs/robot-development',
		title: 'Developing a robot',
		children: [
			{ href: '/docs/robot-development/example', title: 'Example: a first robot' },
			{ href: '/docs/robot-development/project', title: 'Project and code' },
			{ href: '/docs/robot-development/testing', title: 'Running and testing' },
			{ href: '/docs/robot-development/debugging', title: 'Debugging' },
			{ href: '/docs/robot-development/deterministic-mode', title: 'Deterministic mode' },
			{ href: '/docs/robot-development/release', title: 'Packaging and release' }
		]
	},
	{ href: '/docs/ide', title: 'Using an IDE' },
	{
		href: '/docs/cli',
		title: 'Command line',
		children: [
			{ href: '/docs/cli/battle', title: 'battle' },
			{ href: '/docs/cli/replay', title: 'replay' },
			{ href: '/docs/cli/ide-config', title: 'ide-config' },
			{ href: '/docs/cli/agent', title: 'agent' },
			{ href: '/docs/cli/rumble', title: 'rumble' }
		]
	},
	{
		href: '/docs/rumble',
		title: 'The rumble',
		children: [{ href: '/docs/rumble/contributing', title: 'Contributing battles' }]
	},
	{ href: '/docs/about', title: 'About robocode-s' },
	{ href: '/docs/community', title: 'Community and links' }
];
