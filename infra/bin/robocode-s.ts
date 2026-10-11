// CDK app for the robocode-s account (ADR 0016): the certificate in us-east-1 and the site in
// eu-west-1. `pnpm deploy` builds web/ and the sample data first (infra/README.md).

import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { App, Tags } from 'aws-cdk-lib';
import { CertStack } from '../lib/cert-stack.ts';
import { SiteStack } from '../lib/site-stack.ts';

const app = new App();
const domain: string = app.node.getContext('domain');
const account = process.env.CDK_DEFAULT_ACCOUNT;

const siteDir = fileURLToPath(new URL('../../web/build', import.meta.url));
const sampleDataDir = fileURLToPath(new URL('../.sample-data', import.meta.url));
if (!existsSync(`${siteDir}/200.html`)) throw new Error(`${siteDir} missing: run pnpm build:site`);

Tags.of(app).add('project', 'robocode-s');

const cert = new CertStack(app, 'RobocodeS-Cert', {
	env: { account, region: 'us-east-1' },
	domain
});
new SiteStack(app, 'RobocodeS-Site', {
	env: { account, region: 'eu-west-1' },
	domain,
	certificate: cert.certificate,
	siteDir,
	sampleDataDir: existsSync(sampleDataDir) ? sampleDataDir : undefined
});
