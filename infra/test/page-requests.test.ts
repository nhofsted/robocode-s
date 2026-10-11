import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

type Request = { uri: string; querystring: Record<string, { value: string }> };
type Response = { statusCode: number; headers: { location: { value: string } } };

const code = readFileSync(new URL('../lib/page-requests.js', import.meta.url), 'utf8');
const handler = new Function(`${code}; return handler;`)() as (event: {
	request: Request;
}) => Request | Response;

const run = (uri: string, querystring: Request['querystring'] = {}) =>
	handler({ request: { uri, querystring } });
const uriOf = (uri: string) => (run(uri) as Request).uri;

test('home page', () => assert.equal(uriOf('/'), '/index.html'));

test('prerendered pages get .html', () => {
	assert.equal(uriOf('/docs'), '/docs.html');
	assert.equal(uriOf('/docs/getting-started'), '/docs/getting-started.html');
});

test('data pages render from 200.html, also with dots in bot ids', () => {
	assert.equal(uriOf('/rankings'), '/200.html');
	assert.equal(uriOf('/rankings/1v1-micro'), '/200.html');
	assert.equal(uriOf('/bot/sample.Walls'), '/200.html');
	assert.equal(uriOf('/pairing/1v1/a.B/c.D'), '/200.html');
	assert.equal(uriOf('/compare/1v1/a.B/c.D'), '/200.html');
});

test('files pass through', () => {
	assert.equal(uriOf('/data/v1/bots/sample.Walls.json'), '/data/v1/bots/sample.Walls.json');
	assert.equal(uriOf('/data/v1/rankings/1v1.json'), '/data/v1/rankings/1v1.json');
	assert.equal(uriOf('/_app/immutable/entry/start.abc123.js'), '/_app/immutable/entry/start.abc123.js');
	assert.equal(uriOf('/favicon-32.png'), '/favicon-32.png');
	assert.equal(uriOf('/bots'), '/bots.html');
});

test('trailing slash redirects, keeping the query', () => {
	const r = run('/docs/about/', { tab: { value: 'x' }, flag: { value: '' } }) as Response;
	assert.equal(r.statusCode, 301);
	assert.equal(r.headers.location.value, '/docs/about?tab=x&flag');
	assert.equal((run('/rankings/') as Response).headers.location.value, '/rankings');
});
