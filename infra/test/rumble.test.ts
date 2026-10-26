import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { App } from 'aws-cdk-lib';
import { Match, Template } from 'aws-cdk-lib/assertions';
import { Certificate } from 'aws-cdk-lib/aws-certificatemanager';
import { Code } from 'aws-cdk-lib/aws-lambda';
import type { Stage } from '../lib/rumble.ts';
import { SiteStack } from '../lib/site-stack.ts';

// A site directory and placeholder Lambda code: the real ones need pnpm build:site and
// build:lambdas, and the tests must not.
const siteDir = mkdtempSync(`${tmpdir()}/site-`);
mkdirSync(`${siteDir}/_app/immutable`, { recursive: true });
writeFileSync(`${siteDir}/200.html`, '<html></html>');
const lambdaDir = mkdtempSync(`${tmpdir()}/lambda-`);
writeFileSync(`${lambdaDir}/bootstrap`, '#!/bin/sh\n');
const code = () => ({ api: Code.fromAsset(lambdaDir), batch: Code.fromAsset(lambdaDir) });

const synth = (stage: Stage, extra: { alertEmail?: string; devOrigin?: string } = {}) => {
	const app = new App();
	const prod = stage === 'prod';
	const stack = new SiteStack(app, `Site-${stage}`, {
		env: { account: '111111111111', region: 'eu-west-1' },
		stage,
		siteDir,
		githubClientId: 'client-id',
		code: code(),
		...(prod && {
			domain: 'rumble.example.org',
			certificate: Certificate.fromCertificateArn(
				app,
				'Certificate',
				'arn:aws:acm:us-east-1:111111111111:certificate/00000000-0000-0000-0000-000000000000'
			)
		}),
		...extra
	});
	return Template.fromStack(stack);
};

const prod = synth('prod');
const dev = synth('dev');

const statements = (template: Template) =>
	Object.values(template.findResources('AWS::IAM::Policy')).flatMap((policy) => {
		const statement = policy.Properties.PolicyDocument.Statement;
		return Array.isArray(statement) ? statement : [statement];
	});

test('prod table: provisioned 25/25, protected, retained, TTL, no GSIs', () => {
	prod.hasResourceProperties('AWS::DynamoDB::Table', {
		TableName: 'rumble',
		BillingMode: Match.absent(),
		ProvisionedThroughput: { ReadCapacityUnits: 25, WriteCapacityUnits: 25 },
		DeletionProtectionEnabled: true,
		TimeToLiveSpecification: { AttributeName: 'ttl', Enabled: true },
		KeySchema: [
			{ AttributeName: 'pk', KeyType: 'HASH' },
			{ AttributeName: 'sk', KeyType: 'RANGE' }
		],
		AttributeDefinitions: [
			{ AttributeName: 'pk', AttributeType: 'S' },
			{ AttributeName: 'sk', AttributeType: 'S' }
		],
		GlobalSecondaryIndexes: Match.absent()
	});
	prod.hasResource('AWS::DynamoDB::Table', { DeletionPolicy: 'Retain' });
	prod.resourceCountIs('AWS::ApplicationAutoScaling::ScalableTarget', 0);
});

test('dev table: on demand, destroyable', () => {
	dev.hasResourceProperties('AWS::DynamoDB::Table', {
		TableName: 'rumble-dev',
		BillingMode: 'PAY_PER_REQUEST',
		ProvisionedThroughput: Match.absent(),
		DeletionProtectionEnabled: false,
		TimeToLiveSpecification: { AttributeName: 'ttl', Enabled: true },
		GlobalSecondaryIndexes: Match.absent()
	});
	dev.hasResource('AWS::DynamoDB::Table', { DeletionPolicy: 'Delete' });
});

test('buckets: private, encrypted, retained in prod; state versioned with lifecycle and CORS', () => {
	for (const template of [prod, dev]) {
		// Site, packages, jars, state.
		for (const bucket of Object.values(template.findResources('AWS::S3::Bucket'))) {
			assert.deepEqual(bucket.Properties.PublicAccessBlockConfiguration, {
				BlockPublicAcls: true,
				BlockPublicPolicy: true,
				IgnorePublicAcls: true,
				RestrictPublicBuckets: true
			});
		}
	}
	const buckets = Object.values(prod.findResources('AWS::S3::Bucket'));
	assert.equal(buckets.length, 4);
	prod.hasResourceProperties('AWS::S3::Bucket', {
		VersioningConfiguration: { Status: 'Enabled' },
		LifecycleConfiguration: {
			Rules: Match.arrayWith([
				Match.objectLike({ Id: 'uploads', Prefix: 'uploads/', ExpirationInDays: 1, Status: 'Enabled' }),
				Match.objectLike({ Id: 'noncurrent', NoncurrentVersionExpiration: { NoncurrentDays: 7 } })
			])
		},
		CorsConfiguration: {
			CorsRules: [
				{
					AllowedMethods: ['PUT'],
					AllowedOrigins: ['https://rumble.example.org'],
					AllowedHeaders: [
						'content-type',
						'x-amz-checksum-sha256',
						'x-amz-sdk-checksum-algorithm',
						'content-length'
					],
					MaxAge: 3600
				}
			]
		}
	});
	// Only the state bucket has CORS; the jars bucket has none.
	const withCors = buckets.filter((bucket) => bucket.Properties.CorsConfiguration);
	assert.equal(withCors.length, 1);
	for (const bucket of buckets) assert.equal(bucket.DeletionPolicy, 'Retain');
	for (const bucket of Object.values(dev.findResources('AWS::S3::Bucket')).slice(-2)) {
		assert.equal(bucket.DeletionPolicy, 'Delete');
	}
});

test('dev state bucket allows PUT from cloudfront.net, or from devOrigin once known', () => {
	const origins = (template: Template) =>
		Object.values(template.findResources('AWS::S3::Bucket')).flatMap((bucket) =>
			(bucket.Properties.CorsConfiguration?.CorsRules ?? []).flatMap(
				(rule: { AllowedOrigins: string[] }) => rule.AllowedOrigins
			)
		);
	assert.deepEqual(origins(dev), ['https://*.cloudfront.net']);
	assert.deepEqual(origins(synth('dev', { devOrigin: 'https://d111.cloudfront.net' })), [
		'https://d111.cloudfront.net'
	]);
});

test('Lambdas: runtime, memory, timeout, concurrency, environment', () => {
	const functions = Object.values(prod.findResources('AWS::Lambda::Function')).filter(
		(fn) => fn.Properties.Runtime === 'provided.al2023'
	);
	assert.equal(functions.length, 2);
	prod.hasResourceProperties('AWS::Lambda::Function', {
		Runtime: 'provided.al2023',
		Architectures: ['arm64'],
		MemorySize: 256,
		Timeout: 25,
		ReservedConcurrentExecutions: 10,
		Environment: {
			Variables: Match.objectLike({
				STAGE: 'prod',
				TABLE: Match.anyValue(),
				JARS_BUCKET: Match.anyValue(),
				STATE_BUCKET: Match.anyValue(),
				SITE_ORIGIN: 'https://rumble.example.org',
				GITHUB_CLIENT_ID: 'client-id',
				SSM_PREFIX: '/robocode-s/prod'
			})
		}
	});
	prod.hasResourceProperties('AWS::Lambda::Function', {
		Runtime: 'provided.al2023',
		Architectures: ['arm64'],
		MemorySize: 1024,
		Timeout: 900,
		ReservedConcurrentExecutions: 1,
		Environment: {
			Variables: Match.objectLike({
				SITE_BUCKET: Match.anyValue(),
				DISTRIBUTION_ID: Match.anyValue(),
				SSM_PREFIX: '/robocode-s/prod'
			})
		}
	});
});

test('dev: SITE_ORIGIN from context devOrigin, a placeholder before', () => {
	dev.hasResourceProperties('AWS::Lambda::Function', {
		Environment: { Variables: Match.objectLike({ STAGE: 'dev', SITE_ORIGIN: Match.stringLikeRegexp('invalid$') }) }
	});
	synth('dev', { devOrigin: 'https://d111.cloudfront.net' }).hasResourceProperties('AWS::Lambda::Function', {
		Environment: {
			Variables: Match.objectLike({ SITE_ORIGIN: 'https://d111.cloudfront.net', SSM_PREFIX: '/robocode-s/dev' })
		}
	});
});

test('logs: 7 days', () => {
	const groups = Object.values(prod.findResources('AWS::Logs::LogGroup'));
	assert.equal(groups.length, 2);
	for (const group of groups) assert.equal(group.Properties.RetentionInDays, 7);
});

test('function URL: AWS_IAM, buffered; CloudFront may invoke it and the function', () => {
	prod.hasResourceProperties('AWS::Lambda::Url', { AuthType: 'AWS_IAM', InvokeMode: 'BUFFERED' });
	for (const action of ['lambda:InvokeFunctionUrl', 'lambda:InvokeFunction']) {
		prod.hasResourceProperties('AWS::Lambda::Permission', {
			Action: action,
			Principal: 'cloudfront.amazonaws.com',
			SourceArn: Match.objectLike({
				'Fn::Join': Match.arrayWith([Match.arrayWith([Match.stringLikeRegexp(':distribution/')])])
			})
		});
	}
	const permissions = Object.values(prod.findResources('AWS::Lambda::Permission')).filter(
		(permission) => permission.Properties.Principal === 'cloudfront.amazonaws.com'
	);
	assert.deepEqual(
		permissions.map((permission) => permission.Properties.Action).sort(),
		['lambda:InvokeFunction', 'lambda:InvokeFunctionUrl']
	);
});

test('behaviors: api through OAC, jars and rec cached and uncompressed', () => {
	const distribution = Object.values(prod.findResources('AWS::CloudFront::Distribution'))[0];
	const config = distribution.Properties.DistributionConfig;
	const behavior = (pattern: string) =>
		config.CacheBehaviors.find((b: { PathPattern: string }) => b.PathPattern === pattern);
	const cachingDisabled = '4135ea2d-6df8-44a3-9df3-4b5a84be39ad';
	const cachingOptimized = '658327ea-f89d-4fab-a63d-7e88639e58f6';
	const api = behavior('api/*');
	assert.equal(api.CachePolicyId, cachingDisabled);
	assert.deepEqual(api.AllowedMethods, ['GET', 'HEAD', 'OPTIONS', 'PUT', 'PATCH', 'POST', 'DELETE']);
	assert.equal(api.ViewerProtocolPolicy, 'https-only');
	for (const pattern of ['jars/*', 'rec/*']) {
		assert.equal(behavior(pattern).CachePolicyId, cachingOptimized);
		assert.equal(behavior(pattern).Compress, false);
	}
	// Every origin (site, packages, api, jars twice) is reached through an origin access control.
	const origins = config.Origins as { OriginAccessControlId?: unknown }[];
	assert.equal(origins.length, 5);
	assert.equal(origins.filter((origin) => origin.OriginAccessControlId).length, 5);
	prod.hasResourceProperties('AWS::CloudFront::OriginAccessControl', {
		OriginAccessControlConfig: Match.objectLike({ OriginAccessControlOriginType: 'lambda', SigningBehavior: 'always' })
	});
	prod.hasResourceProperties('AWS::CloudFront::OriginRequestPolicy', {
		OriginRequestPolicyConfig: Match.objectLike({
			HeadersConfig: {
				HeaderBehavior: 'whitelist',
				Headers: Match.arrayWith(['x-rumble-token', 'CloudFront-Viewer-Address'])
			},
			CookiesConfig: { CookieBehavior: 'all' },
			QueryStringsConfig: { QueryStringBehavior: 'all' }
		})
	});
});

test('dev distribution has no domain and no certificate', () => {
	const config = Object.values(dev.findResources('AWS::CloudFront::Distribution'))[0].Properties
		.DistributionConfig;
	assert.equal(config.Aliases, undefined);
	assert.equal(config.ViewerCertificate, undefined);
});

test('schedules: plan, publish, cleanup, sync with their inputs', () => {
	prod.resourceCountIs('AWS::Scheduler::Schedule', 4);
	const expected = {
		'rumble-plan': ['cron(12 * * * ? *)', '{"mode":"plan"}'],
		'rumble-publish': ['cron(30 3,9,15,21 * * ? *)', '{"mode":"publish"}'],
		'rumble-cleanup': ['cron(45 4 * * ? *)', '{"mode":"cleanup"}'],
		'rumble-sync': ['cron(15 5 * * ? *)', '{"mode":"sync"}']
	};
	for (const [name, [expression, input]] of Object.entries(expected)) {
		prod.hasResourceProperties('AWS::Scheduler::Schedule', {
			Name: name,
			ScheduleExpression: expression,
			Target: Match.objectLike({ Input: input })
		});
	}
	dev.hasResourceProperties('AWS::Scheduler::Schedule', { Name: 'rumble-dev-plan' });
});

test('IAM: no wildcard resources except kms:Decrypt through ssm', () => {
	for (const template of [prod, dev]) {
		for (const statement of statements(template)) {
			const resources = [statement.Resource].flat();
			if (!resources.includes('*')) continue;
			const actions = [statement.Action].flat();
			// Not the rumble's: the deployment handlers' and the like.
			if (actions.every((action: string) => !action.startsWith('kms:Decrypt'))) continue;
			assert.deepEqual(actions, ['kms:Decrypt']);
			assert.deepEqual(statement.Condition, {
				StringEquals: { 'kms:ViaService': 'ssm.eu-west-1.amazonaws.com' }
			});
		}
	}
	const rumble = JSON.stringify(statements(prod));
	// The batch Lambda invalidates only this distribution; SSM reads stay under the stage's prefix.
	assert.match(rumble, /cloudfront:CreateInvalidation/);
	assert.match(rumble, /parameter\/robocode-s\/prod\/\*/);
	assert.doesNotMatch(rumble, /"s3:\*"|"dynamodb:\*"|"ssm:\*"/);
});

test('IAM: api cannot touch state outside config, plan and uploads, nor delete jars', () => {
	const s3 = statements(prod).filter((statement) =>
		[statement.Action].flat().some((action: string) => action.startsWith('s3:'))
	);
	const forApi = JSON.stringify(
		Object.values(prod.findResources('AWS::IAM::Policy'))
			.filter((policy) => JSON.stringify(policy).includes('ApiServiceRole'))
			.flatMap((policy) => policy.Properties.PolicyDocument.Statement)
	);
	assert.ok(s3.length > 0);
	assert.match(forApi, /state\/config\.json/);
	assert.match(forApi, /uploads\/\*/);
	assert.match(forApi, /jars\/\*/);
	assert.match(forApi, /rec\/\*/);
	assert.doesNotMatch(forApi, /BatchWriteItem/);
	// DeleteObject on jars/* is the batch Lambda's alone.
	for (const statement of JSON.parse(forApi)) {
		const actions = [statement.Action].flat();
		if (!actions.includes('s3:DeleteObject')) continue;
		assert.doesNotMatch(JSON.stringify(statement.Resource), /jars|rec\//);
	}
});

test('IAM: batch puts jars (sync), but no recordings', () => {
	const forBatch = Object.values(prod.findResources('AWS::IAM::Policy'))
		.filter((policy) => JSON.stringify(policy).includes('BatchServiceRole'))
		.flatMap((policy) => policy.Properties.PolicyDocument.Statement);
	const puts = forBatch.filter(
		(statement: { Action: string | string[]; Resource: unknown }) =>
			[statement.Action].flat().includes('s3:PutObject') &&
			JSON.stringify(statement.Resource).includes('JarsBucket')
	);
	assert.equal(puts.length, 1);
	assert.match(JSON.stringify(puts[0].Resource), /jars\/\*/);
	assert.doesNotMatch(JSON.stringify(puts[0].Resource), /rec\//);
});

test('alarms: ALERT filters, throttles, and SNS only with alertEmail', () => {
	for (const template of [prod, dev]) {
		template.resourceCountIs('AWS::Logs::MetricFilter', 2);
		template.hasResourceProperties('AWS::Logs::MetricFilter', { FilterPattern: '"ALERT"' });
		template.resourceCountIs('AWS::CloudWatch::Alarm', 4);
		template.hasResourceProperties('AWS::CloudWatch::Alarm', {
			MetricName: 'Throttles',
			Namespace: 'AWS/Lambda',
			Threshold: 0,
			Period: 900
		});
		template.hasResourceProperties('AWS::CloudWatch::Alarm', {
			MetricName: 'WriteThrottleEvents',
			Namespace: 'AWS/DynamoDB',
			Threshold: 100,
			Period: 900
		});
		template.resourceCountIs('AWS::SNS::Topic', 0);
		template.resourceCountIs('AWS::SNS::Subscription', 0);
	}
	const withEmail = synth('prod', { alertEmail: 'owner@example.org' });
	withEmail.resourceCountIs('AWS::SNS::Topic', 1);
	withEmail.hasResourceProperties('AWS::SNS::Subscription', {
		Protocol: 'email',
		Endpoint: 'owner@example.org'
	});
	for (const alarm of Object.values(withEmail.findResources('AWS::CloudWatch::Alarm'))) {
		assert.equal(alarm.Properties.AlarmActions.length, 1);
	}
});

test('outputs', () => {
	for (const name of ['ApiFunctionUrl', 'JarsBucketName', 'StateBucketName', 'TableName']) {
		assert.ok(Object.keys(prod.toJSON().Outputs).some((key) => key.includes(name)), name);
	}
});
