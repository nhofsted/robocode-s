import { loadContributors } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => ({ contributors: await loadContributors(fetch) });
