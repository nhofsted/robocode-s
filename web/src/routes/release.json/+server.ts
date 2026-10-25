// The current release for the programs' update check (src/lib/release.ts).
import { json } from '@sveltejs/kit';
import { releaseJson } from '#lib/release.ts';

export const prerender = true;

export const GET = () => json(releaseJson());
