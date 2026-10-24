import { loadArchive } from '#lib/rumble/data.ts';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => ({ archive: await loadArchive(fetch) });
