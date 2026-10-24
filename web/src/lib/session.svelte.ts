// The browser's sign-in state, shared by the header and the account pages.
import { ApiError, getMe, logout, type Me } from './api.ts';

export type SessionState = 'loading' | 'in' | 'out' | 'unavailable';

class Session {
	state: SessionState = $state('loading');
	me: Me | null = $state(null);
	#loading: Promise<void> | undefined;

	/** Asks the server who is signed in (once, unless `force`). */
	load(force = false): Promise<void> {
		if (this.#loading && !force) return this.#loading;
		this.#loading = getMe().then(
			(me) => {
				this.me = me;
				this.state = 'in';
			},
			(e: unknown) => {
				this.me = null;
				// 401: not signed in. Anything else (no backend in dev, network): say so.
				this.state = e instanceof ApiError && !e.unavailable ? 'out' : 'unavailable';
			}
		);
		return this.#loading;
	}

	async signOut(): Promise<void> {
		try {
			await logout();
		} catch {
			// The cookie may be gone already; the page reloads signed out either way.
		}
		this.me = null;
		this.state = 'out';
		this.#loading = undefined;
	}
}

export const session = new Session();
