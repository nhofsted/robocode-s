import { describe, expect, it } from 'vitest';
import { ApiError, getMe, loginUrl, putUpload, retireBot, sha256Hex, logout, getMyBots } from './api.ts';

type Call = { url: string; init: RequestInit };

function fake(response: Response | Error) {
	const calls: Call[] = [];
	const fetch = (async (url: string, init: RequestInit) => {
		calls.push({ url, init });
		if (response instanceof Error) throw response;
		return response.clone();
	}) as typeof globalThis.fetch;
	return { calls, fetch };
}

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
	new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...headers } });

describe('sha256Hex', () => {
	it('hashes the empty string and "abc"', async () => {
		expect(await sha256Hex(new Uint8Array())).toBe(
			'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
		);
		expect(await sha256Hex(new TextEncoder().encode('abc'))).toBe(
			'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
		);
	});
});

describe('POST hash header', () => {
	it('is the SHA-256 of the exact body bytes', async () => {
		const { calls, fetch } = fake(json(200, {}));
		await retireBot('a.B', { fetch });
		const { init, url } = calls[0];
		expect(url).toBe('/api/v1/bots/retire');
		expect(init.method).toBe('POST');
		expect(init.credentials).toBe('same-origin');
		const body = init.body as Uint8Array<ArrayBuffer>;
		expect(new TextDecoder().decode(body)).toBe('{"bot":"a.B"}');
		expect((init.headers as Record<string, string>)['x-amz-content-sha256']).toBe(await sha256Hex(body));
		expect((init.headers as Record<string, string>)['content-type']).toBe('application/json');
	});

	it('is the hash of the empty string for a POST without a body', async () => {
		const { calls, fetch } = fake(new Response(null, { status: 204 }));
		await logout({ fetch });
		const headers = calls[0].init.headers as Record<string, string>;
		expect(calls[0].init.body).toBeUndefined();
		expect(headers['x-amz-content-sha256']).toBe(
			'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
		);
	});

	it('is not sent on GET', async () => {
		const { calls, fetch } = fake(json(200, { login: 'x' }));
		await getMe({ fetch });
		expect((calls[0].init.headers as Record<string, string>)['x-amz-content-sha256']).toBeUndefined();
	});
});

describe('error mapping', () => {
	it('maps the error body', async () => {
		const { fetch } = fake(json(403, { error: 'forbidden', message: 'Ask the owner.' }));
		const e = await getMe({ fetch }).catch((e) => e);
		expect(e).toBeInstanceOf(ApiError);
		expect(e).toMatchObject({ status: 403, code: 'forbidden', message: 'Ask the owner.' });
		expect(e.unavailable).toBe(false);
	});

	it('maps 401 to the signed-out state', async () => {
		const { fetch } = fake(json(401, { error: 'unauthorized', message: 'Sign in.' }));
		const e = await getMe({ fetch }).catch((e) => e);
		expect(e.signedOut).toBe(true);
	});

	it('keeps Retry-After', async () => {
		const { fetch } = fake(json(429, { error: 'rate_limited', message: 'Slow down.' }, { 'retry-after': '12' }));
		const e = await getMe({ fetch }).catch((e) => e);
		expect(e).toMatchObject({ code: 'rate_limited', retryAfter: 12 });
	});

	it('copes with bodies that are not JSON (no backend)', async () => {
		const { fetch } = fake(new Response('<html>nope</html>', { status: 404 }));
		const e = await getMe({ fetch }).catch((e) => e);
		expect(e).toMatchObject({ status: 404, code: 'http_404' });
		expect(e.unavailable).toBe(true);
	});

	it('treats a 200 that is not JSON as unavailable', async () => {
		const { fetch } = fake(new Response('<html></html>', { status: 200 }));
		const e = await getMe({ fetch }).catch((e) => e);
		expect(e.unavailable).toBe(true);
	});

	it('maps network failures to status 0', async () => {
		const { fetch } = fake(new TypeError('failed'));
		const e = await getMe({ fetch }).catch((e) => e);
		expect(e).toMatchObject({ status: 0, code: 'network' });
		expect(e.unavailable).toBe(true);
	});
});

describe('lists and uploads', () => {
	it('accepts a bare array or a wrapped one', async () => {
		expect(await getMyBots({ fetch: fake(json(200, [{ id: 'a.B' }])).fetch })).toHaveLength(1);
		expect(await getMyBots({ fetch: fake(json(200, { bots: [{ id: 'a.B' }] })).fetch })).toHaveLength(1);
	});

	it('PUTs with the signed headers except Content-Length, without cookies', async () => {
		const { calls, fetch } = fake(new Response(null, { status: 200 }));
		const upload = {
			url: 'https://bucket.s3.amazonaws.com/uploads/u/x.jar?X-Amz=1',
			headers: { 'Content-Length': '3', 'x-amz-checksum-sha256': 'abc=' },
			expires: ''
		};
		await putUpload(upload, new Blob(['abc']), { fetch });
		expect(calls[0].init.method).toBe('PUT');
		expect(calls[0].init.credentials).toBe('omit');
		expect(calls[0].init.headers).toEqual({ 'x-amz-checksum-sha256': 'abc=' });
	});

	it('reports a refused upload', async () => {
		const { fetch } = fake(new Response('denied', { status: 403 }));
		const e = await putUpload({ url: 'https://x', headers: {}, expires: '' }, new Blob(['a']), { fetch }).catch((e) => e);
		expect(e).toMatchObject({ status: 403, code: 'upload_failed' });
	});
});

describe('loginUrl', () => {
	it('encodes the return path', () => {
		expect(loginUrl('/account/bots?x=1')).toBe('/api/v1/auth/login?return=%2Faccount%2Fbots%3Fx%3D1');
	});
});
