// CDK app for the robocode-s account (ADR 0016): the certificate in us-east-1 and the site in
// eu-west-1; `-c stage=dev` instead deploys the dev site alone. `pnpm deploy` builds web/, the
// sample data and the Lambdas first (infra/README.md).

import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { App, Tags } from 'aws-cdk-lib';
import { Code } from 'aws-cdk-lib/aws-lambda';
import { CertStack } from '../lib/cert-stack.ts';
import { lambdaAssetDir, type Stage } from '../lib/rumble.ts';
import { SiteStack } from '../lib/site-stack.ts';

const app = new App();
const account = process.env.CDK_DEFAULT_ACCOUNT;

const stage: Stage = app.node.tryGetContext('stage') ?? 'prod';
if (stage !== 'prod' && stage !== 'dev') throw new Error(`-c stage=${stage}: prod or dev`);

const context = (name: string): string | undefined => app.node.tryGetContext(name) || undefined;
const required = (name: string): string => {
	const value = context(name);
	if (!value) throw new Error(`context ${name} missing: -c ${name}=... (infra/README.md)`);
	return value;
};

const siteDir = fileURLToPath(new URL('../../web/build', import.meta.url));
const sampleDataDir = fileURLToPath(new URL('../.sample-data', import.meta.url));
if (!existsSync(`${siteDir}/200.html`)) throw new Error(`${siteDir} missing: run pnpm build:site`);

const lambdaAsset = (name: string) => {
	const dir = lambdaAssetDir(name);
	if (!existsSync(`${dir}/bootstrap`)) throw new Error(`${dir}/bootstrap missing: run pnpm build:lambdas`);
	return Code.fromAsset(dir);
};
const code = { api: lambdaAsset('api'), batch: lambdaAsset('batch') };

// The sample data deployment prunes data/v1/, which would delete the files `batch publish` wrote:
// on by default in dev only. Turn it off in prod with the first publish (`-c sampleData=false` is
// the default there); turn it on to reseed with `-c sampleData=true`.
const sampleData = (context('sampleData') ?? String(stage === 'dev')) === 'true';

Tags.of(app).add('project', 'robocode-s');

const site = {
	siteDir,
	sampleDataDir: sampleData && existsSync(sampleDataDir) ? sampleDataDir : undefined,
	alertEmail: context('alertEmail'),
	code
};

if (stage === 'prod') {
	const domain: string = app.node.getContext('domain');
	const cert = new CertStack(app, 'RobocodeS-Cert', {
		env: { account, region: 'us-east-1' },
		domain
	});
	new SiteStack(app, 'RobocodeS-Site', {
		...site,
		env: { account, region: 'eu-west-1' },
		stage,
		domain,
		certificate: cert.certificate,
		githubClientId: required('githubClientId')
	});
} else {
	// No domain, no certificate: CloudFront's own domain. Its origin is only known after the first
	// deploy: pass it as -c devOrigin=https://dxxxx.cloudfront.net on the second (infra/README.md).
	new SiteStack(app, 'RobocodeS-Dev-Site', {
		...site,
		env: { account, region: 'eu-west-1' },
		stage,
		devOrigin: context('devOrigin'),
		githubClientId: required('githubClientIdDev')
	});
}
