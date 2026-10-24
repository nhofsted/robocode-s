// The one fetch wrapper for the rumble's HTTP API (docs/rumble-backend.md §5): same-origin
// cookie session, JSON, the body hash CloudFront's origin needs on every POST (§5.1) and the
// error body mapped to ApiError. Base URL: VITE_RUMBLE_API, default /api/v1.

export const API_BASE: string = import.meta.env.VITE_RUMBLE_API ?? '/api/v1';

type Fetch = typeof fetch;

export type Tier = 'new' | 'verifier' | 'quarantined' | 'banned';

export interface Me {
	uid: string;
	login: string;
	name: string | null;
	country: string | null;
	/** Hidden from the public contributors list (`POST me` `{hidden}`). */
	hidden?: boolean;
	reputation: number;
	tier: Tier;
	verifyRate: number;
	battles: number;
	verifiedMatches: number;
	mismatches: number;
	admin: boolean;
	since: string;
}

export interface TokenInfo {
	id: string;
	/** `upload` for the CLI's and GUI's tokens, `session` for browser sessions. */
	kind: 'upload' | 'session';
	label: string | null;
	created: string;
	lastUsed: string | null;
	/** The token or session making this request. */
	current?: boolean;
}

export type VersionState = 'probation' | 'active' | 'failed' | 'retired' | 'removed';

export interface BotVersionInfo {
	version: string;
	vid: number;
	state: VersionState;
	codesize?: number;
	weightClass?: string;
	created?: string;
	/** Probation progress: battles run and failed. */
	probation?: { battles: number; failed: number };
	/** Why a version failed: the clients' failure details. */
	failures?: string[];
}

export interface BotInfo {
	id: string;
	state: VersionState;
	versions: BotVersionInfo[];
}

export interface DivisionInfo {
	id: string;
	rounds: number;
	battlefield: { width: number; height: number };
	robots: number;
}

export interface Info {
	api: number;
	engineVersion: number;
	minClient: string;
	paused: boolean;
	divisions: DivisionInfo[];
	downloads?: string;
}

export interface Upload {
	url: string;
	headers: Record<string, string>;
	expires: string;
}

export type SubmitSource = { source: 'upload'; sha256: string } | { source: 'url'; url: string };

export interface SubmitRequest {
	source: SubmitSource;
	divisions: string[];
	homepage?: string;
}

export interface SubmitResult {
	bot: string;
	version: string;
	vid: number;
	state: VersionState;
	codesize: number;
	weightClass: string;
}

/** An API failure: `status` 0 means the request never got an answer. */
export class ApiError extends Error {
	readonly status: number;
	readonly code: string;
	readonly retryAfter: number | null;

	constructor(status: number, code: string, message: string, retryAfter: number | null = null) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.code = code;
		this.retryAfter = retryAfter;
	}

	/** True for 401: no session. */
	get signedOut(): boolean {
		return this.status === 401;
	}

	/** True when there is no API behind this page (dev without a backend, network down). */
	get unavailable(): boolean {
		return this.status === 0 || this.status === 404 || this.status >= 500;
	}
}

const hex = (bytes: ArrayBuffer) =>
	Array.from(new Uint8Array(bytes), (b) => b.toString(16).padStart(2, '0')).join('');

/** Lower-case hex SHA-256, as `x-amz-content-sha256` and the jar hashes want it. */
export async function sha256Hex(data: BufferSource): Promise<string> {
	return hex(await crypto.subtle.digest('SHA-256', data));
}

async function failure(response: Response): Promise<ApiError> {
	const retry = Number(response.headers.get('retry-after'));
	const retryAfter = Number.isFinite(retry) && retry > 0 ? retry : null;
	let code = `http_${response.status}`;
	let message = `The rumble server answered HTTP ${response.status}.`;
	try {
		const body = (await response.json()) as { error?: unknown; message?: unknown };
		if (typeof body.error === 'string') code = body.error;
		if (typeof body.message === 'string' && body.message) message = body.message;
	} catch {
		// Not our JSON (an HTML 404 from the dev server, a gateway error): keep the generic text.
	}
	return new ApiError(response.status, code, message, retryAfter);
}

export interface Options {
	fetch?: Fetch;
}

async function request<T>(
	method: 'GET' | 'POST',
	path: string,
	body: unknown,
	options: Options
): Promise<T> {
	const doFetch = options.fetch ?? fetch;
	const headers: Record<string, string> = { accept: 'application/json' };
	let bytes: Uint8Array<ArrayBuffer> | undefined;
	if (method === 'POST') {
		bytes = body === undefined ? undefined : new TextEncoder().encode(JSON.stringify(body));
		// The hash of the exact bytes sent; of the empty string when there is no body (§5.1).
		headers['x-amz-content-sha256'] = await sha256Hex(bytes ?? new Uint8Array());
		if (bytes) headers['content-type'] = 'application/json';
	}
	let response: Response;
	try {
		response = await doFetch(`${API_BASE}/${path}`, {
			method,
			headers,
			body: bytes,
			credentials: 'same-origin'
		});
	} catch {
		throw new ApiError(0, 'network', 'Could not reach the rumble server. Check your connection and try again.');
	}
	if (!response.ok) throw await failure(response);
	if (response.status === 204) return undefined as T;
	try {
		return (await response.json()) as T;
	} catch {
		// 200 with a body that isn't JSON: most likely the site's own fallback page.
		throw new ApiError(404, 'unavailable', 'The rumble server is not available here.');
	}
}

const get = <T>(path: string, options: Options = {}) => request<T>('GET', path, undefined, options);
const post = <T>(path: string, body: unknown, options: Options = {}) =>
	request<T>('POST', path, body, options);

/** Where the browser goes to sign in with GitHub, and comes back to `returnPath`. */
export const loginUrl = (returnPath: string) =>
	`${API_BASE}/auth/login?return=${encodeURIComponent(returnPath)}`;

export const getInfo = (o?: Options) => get<Info>('info', o);
export const getMe = (o?: Options) => get<Me>('me', o);
export const updateMe = (change: { country?: string | null; hidden?: boolean }, o?: Options) =>
	post<Me | undefined>('me', change, o);
export const logout = (o?: Options) => post<undefined>('auth/logout', undefined, o);

const list = <T>(value: T[] | Record<string, T[]>, key: string): T[] =>
	Array.isArray(value) ? value : (value[key] ?? []);

export const getTokens = async (o?: Options) =>
	list(await get<TokenInfo[] | { tokens: TokenInfo[] }>('me/tokens', o), 'tokens');
export const revokeToken = (id: string, o?: Options) =>
	post<undefined>('me/tokens/revoke', { id }, o);

export const getMyBots = async (o?: Options) =>
	list(await get<BotInfo[] | { bots: BotInfo[] }>('me/bots', o), 'bots');
export const retireBot = (bot: string, o?: Options) => post<undefined>('bots/retire', { bot }, o);

export const getUploadUrl = (sha256: string, size: number, o?: Options) =>
	post<Upload>('bots/upload-url', { sha256, size }, o);
export const submitBot = (request: SubmitRequest, o?: Options) =>
	post<SubmitResult>(
		'bots/submit',
		{ ...request.source, divisions: request.divisions, homepage: request.homepage || undefined },
		o
	);

/**
 * PUTs the jar to the presigned URL with exactly the headers the server signed. The browser
 * sets Content-Length itself (and refuses it as a header); it is the length that was signed.
 */
export async function putUpload(upload: Upload, jar: Blob, o: Options = {}): Promise<void> {
	const headers = Object.fromEntries(
		Object.entries(upload.headers).filter(([name]) => name.toLowerCase() !== 'content-length')
	);
	let response: Response;
	try {
		response = await (o.fetch ?? fetch)(upload.url, {
			method: 'PUT',
			headers,
			body: jar,
			credentials: 'omit'
		});
	} catch {
		throw new ApiError(0, 'network', 'The upload failed. Check your connection and try again.');
	}
	if (!response.ok) {
		throw new ApiError(
			response.status,
			'upload_failed',
			`The upload was refused (HTTP ${response.status}). The upload link may have expired: try again.`
		);
	}
}
