// The rumble's backend (docs/rumble-backend.md §14.2): table, buckets, the `api` and `batch`
// Lambdas, the CloudFront behaviors in front of them, schedules and alarms. It lives in the site's
// stack because the behaviors, the OAC grants and the publisher's write access to the site bucket
// would otherwise make cross-stack cycles.

import { CfnOutput, Duration, RemovalPolicy, Stack } from 'aws-cdk-lib';
import {
	AllowedMethods,
	CachePolicy,
	type Distribution,
	OriginRequestCookieBehavior,
	OriginRequestHeaderBehavior,
	OriginRequestPolicy,
	OriginRequestQueryStringBehavior,
	ResponseHeadersPolicy,
	ViewerProtocolPolicy
} from 'aws-cdk-lib/aws-cloudfront';
import { FunctionUrlOrigin, S3BucketOrigin } from 'aws-cdk-lib/aws-cloudfront-origins';
import { Alarm, ComparisonOperator, Metric, TreatMissingData } from 'aws-cdk-lib/aws-cloudwatch';
import { SnsAction } from 'aws-cdk-lib/aws-cloudwatch-actions';
import { AttributeType, BillingMode, Table } from 'aws-cdk-lib/aws-dynamodb';
import { Effect, PolicyStatement } from 'aws-cdk-lib/aws-iam';
import {
	Architecture,
	type Code,
	Function as LambdaFunction,
	FunctionUrlAuthType,
	InvokeMode,
	Runtime
} from 'aws-cdk-lib/aws-lambda';
import { FilterPattern, LogGroup, MetricFilter, RetentionDays } from 'aws-cdk-lib/aws-logs';
import {
	BlockPublicAccess,
	Bucket,
	BucketEncryption,
	HttpMethods,
	type IBucket
} from 'aws-cdk-lib/aws-s3';
import { Schedule, ScheduleExpression, ScheduleTargetInput } from 'aws-cdk-lib/aws-scheduler';
import { LambdaInvoke } from 'aws-cdk-lib/aws-scheduler-targets';
import { Topic } from 'aws-cdk-lib/aws-sns';
import { EmailSubscription } from 'aws-cdk-lib/aws-sns-subscriptions';
import { Construct } from 'constructs';

export type Stage = 'prod' | 'dev';

export interface RumbleProps {
	stage: Stage;
	distribution: Distribution;
	/** The site bucket; `batch publish` writes `data/v1/`. */
	siteBucket: IBucket;
	/**
	 * `https://<domain>`, as the browser sees it: the `api` Lambda's `SITE_ORIGIN` (CSRF check,
	 * redirects). In dev the distribution's own domain can't be read by the Lambda (the distribution
	 * points at the Lambda: a dependency cycle), so it is a context value (infra/README.md).
	 */
	siteOrigin: string;
	/** The origin the browser uploads jars from (CORS of the state bucket). */
	corsOrigin: string;
	githubClientId: string;
	/** E-mail for the alarms. Without it there is no topic; the alarms only show in the console. */
	alertEmail?: string;
	/** The Lambdas' code: `lambdaAsset('api')` and `lambdaAsset('batch')`. */
	code: { api: Code; batch: Code };
}

/** Where `tools/rumble/build-lambdas.sh` puts the binaries. */
export const lambdaAssetDir = (name: string) =>
	new URL(`../../rumble/lambda/target/lambda/${name}`, import.meta.url).pathname;

export class Rumble extends Construct {
	readonly table: Table;
	readonly jarsBucket: Bucket;
	readonly stateBucket: Bucket;
	readonly api: LambdaFunction;
	readonly batch: LambdaFunction;
	readonly alertTopic?: Topic;

	constructor(scope: Construct, id: string, props: RumbleProps) {
		super(scope, id);
		const stack = Stack.of(this);
		const prod = props.stage === 'prod';
		const removal = prod ? RemovalPolicy.RETAIN : RemovalPolicy.DESTROY;
		const suffix = prod ? '' : `-${props.stage}`;
		const ssmPrefix = `/robocode-s/${props.stage}`;

		// §9.1: no GSIs, no autoscaling, no point-in-time recovery (it costs).
		this.table = new Table(this, 'Table', {
			tableName: `rumble${suffix}`,
			partitionKey: { name: 'pk', type: AttributeType.STRING },
			sortKey: { name: 'sk', type: AttributeType.STRING },
			timeToLiveAttribute: 'ttl',
			...(prod
				? { billingMode: BillingMode.PROVISIONED, readCapacity: 25, writeCapacity: 25 }
				: { billingMode: BillingMode.PAY_PER_REQUEST }),
			deletionProtection: prod,
			removalPolicy: removal
		});

		// §8: two buckets so that CloudFront's OAC grant can never expose state or uploads.
		const bucketDefaults = {
			blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
			encryption: BucketEncryption.S3_MANAGED,
			enforceSSL: true,
			removalPolicy: removal,
			autoDeleteObjects: !prod
		};
		this.jarsBucket = new Bucket(this, 'JarsBucket', bucketDefaults);
		this.stateBucket = new Bucket(this, 'StateBucket', {
			...bucketDefaults,
			versioned: true,
			lifecycleRules: [
				{ id: 'uploads', prefix: 'uploads/', expiration: Duration.days(1) },
				{ id: 'noncurrent', noncurrentVersionExpiration: Duration.days(7) }
			],
			// The website uploads jars from the browser with presigned PUTs to `uploads/`.
			cors: [
				{
					allowedMethods: [HttpMethods.PUT],
					allowedOrigins: [props.corsOrigin],
					allowedHeaders: [
						'content-type',
						'x-amz-checksum-sha256',
						'x-amz-sdk-checksum-algorithm',
						'content-length'
					],
					maxAge: 3600
				}
			]
		});

		const logGroup = (name: string) =>
			new LogGroup(this, `${name}Logs`, {
				retention: RetentionDays.ONE_WEEK,
				removalPolicy: RemovalPolicy.DESTROY
			});
		const apiLogs = logGroup('Api');
		const batchLogs = logGroup('Batch');

		const lambdaDefaults = {
			runtime: Runtime.PROVIDED_AL2023,
			architecture: Architecture.ARM_64,
			handler: 'bootstrap'
		};
		this.api = new LambdaFunction(this, 'Api', {
			...lambdaDefaults,
			code: props.code.api,
			memorySize: 256,
			timeout: Duration.seconds(25),
			reservedConcurrentExecutions: 10,
			logGroup: apiLogs,
			environment: {
				STAGE: props.stage,
				TABLE: this.table.tableName,
				JARS_BUCKET: this.jarsBucket.bucketName,
				STATE_BUCKET: this.stateBucket.bucketName,
				SITE_ORIGIN: props.siteOrigin,
				GITHUB_CLIENT_ID: props.githubClientId,
				SSM_PREFIX: ssmPrefix,
				RUST_LOG: 'info'
			}
		});
		this.batch = new LambdaFunction(this, 'Batch', {
			...lambdaDefaults,
			code: props.code.batch,
			memorySize: 1024,
			timeout: Duration.seconds(900),
			reservedConcurrentExecutions: 1,
			logGroup: batchLogs,
			environment: {
				STAGE: props.stage,
				TABLE: this.table.tableName,
				JARS_BUCKET: this.jarsBucket.bucketName,
				STATE_BUCKET: this.stateBucket.bucketName,
				SITE_BUCKET: props.siteBucket.bucketName,
				DISTRIBUTION_ID: props.distribution.distributionId,
				SSM_PREFIX: ssmPrefix,
				RUST_LOG: 'info'
			}
		});

		// IAM. To give a Lambda more, add a line here: `allow(fn, actions, resources)`.
		const allow = (fn: LambdaFunction, actions: string[], resources: string[]) =>
			fn.addToRolePolicy(new PolicyStatement({ effect: Effect.ALLOW, actions, resources }));
		const keys = (bucket: IBucket, ...prefixes: string[]) =>
			prefixes.map((prefix) => bucket.arnForObjects(prefix));

		this.table.grant(
			this.api,
			'dynamodb:GetItem',
			'dynamodb:Query',
			'dynamodb:PutItem',
			'dynamodb:UpdateItem',
			'dynamodb:DeleteItem',
			'dynamodb:TransactWriteItems',
			'dynamodb:BatchGetItem'
		);
		this.table.grant(
			this.batch,
			'dynamodb:GetItem',
			'dynamodb:Query',
			'dynamodb:PutItem',
			'dynamodb:UpdateItem',
			'dynamodb:DeleteItem',
			'dynamodb:BatchWriteItem',
			'dynamodb:BatchGetItem'
		);

		// api: the configuration and the plan (read), browser uploads (read, presign, delete).
		allow(
			this.api,
			['s3:GetObject'],
			keys(this.stateBucket, 'state/config.json', 'state/*/plan.json.gz')
		);
		allow(
			this.api,
			['s3:GetObject', 's3:PutObject', 's3:DeleteObject'],
			keys(this.stateBucket, 'uploads/*')
		);
		// api: jars (exists checks, re-validating imports) and recordings.
		allow(this.api, ['s3:GetObject', 's3:PutObject'], keys(this.jarsBucket, 'jars/*', 'rec/*'));
		// Without s3:ListBucket a missing object answers 403 instead of 404.
		allow(this.api, ['s3:ListBucket'], [this.jarsBucket.bucketArn, this.stateBucket.bucketArn]);

		// batch: the state, the published data, takedowns and cleanup.
		allow(
			this.batch,
			['s3:GetObject', 's3:PutObject', 's3:DeleteObject'],
			keys(this.stateBucket, '*')
		);
		allow(
			this.batch,
			['s3:GetObject', 's3:PutObject', 's3:DeleteObject'],
			keys(props.siteBucket, 'data/*')
		);
		allow(this.batch, ['s3:GetObject', 's3:DeleteObject'], keys(this.jarsBucket, 'jars/*', 'rec/*'));
		allow(
			this.batch,
			['s3:ListBucket'],
			[this.stateBucket.bucketArn, props.siteBucket.bucketArn, this.jarsBucket.bucketArn]
		);
		allow(this.batch, ['cloudfront:CreateInvalidation'], [props.distribution.distributionArn]);

		// SSM SecureStrings under the stage's prefix, decrypted by the AWS-managed aws/ssm key (its
		// ARN isn't known to CDK, hence `*`, narrowed by the ViaService condition).
		const parameters = stack.formatArn({
			service: 'ssm',
			resource: 'parameter',
			resourceName: `${ssmPrefix.slice(1)}/*`
		});
		for (const fn of [this.api, this.batch]) {
			allow(fn, ['ssm:GetParameter', 'ssm:GetParameters'], [parameters]);
			fn.addToRolePolicy(
				new PolicyStatement({
					actions: ['kms:Decrypt'],
					resources: ['*'],
					conditions: { StringEquals: { 'kms:ViaService': `ssm.${stack.region}.amazonaws.com` } }
				})
			);
		}

		// §5.1: CloudFront signs every origin request to the function URL (OAC).
		const url = this.api.addFunctionUrl({
			authType: FunctionUrlAuthType.AWS_IAM,
			invokeMode: InvokeMode.BUFFERED
		});
		// The managed ALL_VIEWER_EXCEPT_HOST_HEADER policy can't be extended, and forwarding Host would
		// break the function URL, so the policy lists what the API reads (cookies and query strings
		// all; Authorization never arrives, OAC overwrites it). x-amz-content-sha256 can't be listed
		// (CloudFront refuses it): OAC uses the viewer's header itself when it signs the request.
		const apiRequests = new OriginRequestPolicy(this, 'ApiRequests', {
			comment: 'rumble api: viewer headers the Lambda reads, no Host',
			cookieBehavior: OriginRequestCookieBehavior.all(),
			queryStringBehavior: OriginRequestQueryStringBehavior.all(),
			headerBehavior: OriginRequestHeaderBehavior.allowList(
				'accept',
				'content-type',
				'origin',
				'user-agent',
				'x-rumble-client',
				'x-rumble-token',
				'CloudFront-Viewer-Address'
			)
		});
		props.distribution.addBehavior('api/*', FunctionUrlOrigin.withOriginAccessControl(url), {
			viewerProtocolPolicy: ViewerProtocolPolicy.HTTPS_ONLY,
			allowedMethods: AllowedMethods.ALLOW_ALL,
			cachePolicy: CachePolicy.CACHING_DISABLED,
			originRequestPolicy: apiRequests,
			responseHeadersPolicy: ResponseHeadersPolicy.SECURITY_HEADERS
		});
		// CDK's OAC grants cloudfront.amazonaws.com both lambda:InvokeFunctionUrl and (since October
		// 2025, required for function URLs) lambda:InvokeFunction, with the distribution as SourceArn;
		// test/rumble.test.ts checks it stays so.

		// Jars (already zipped) and recordings (range-coded): no compression.
		for (const prefix of ['jars/*', 'rec/*']) {
			props.distribution.addBehavior(
				prefix,
				S3BucketOrigin.withOriginAccessControl(this.jarsBucket),
				{
					viewerProtocolPolicy: ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
					cachePolicy: CachePolicy.CACHING_OPTIMIZED,
					responseHeadersPolicy: ResponseHeadersPolicy.SECURITY_HEADERS,
					compress: false
				}
			);
		}

		// UTC. Plan hourly at :05, publish every 6 hours (it plans first), cleanup daily.
		const schedule = (mode: string, minute: string, hour: string) =>
			new Schedule(this, `${mode[0].toUpperCase()}${mode.slice(1)}Schedule`, {
				scheduleName: `rumble${suffix}-${mode}`,
				schedule: ScheduleExpression.cron({ minute, hour }),
				target: new LambdaInvoke(this.batch, {
					input: ScheduleTargetInput.fromObject({ mode }),
					retryAttempts: 0
				})
			});
		schedule('plan', '5', '*');
		schedule('publish', '30', '3,9,15,21');
		schedule('cleanup', '45', '4');

		// Alarms. The code logs `ALERT ...` lines for what the owner must look at (§11.4).
		if (props.alertEmail) {
			this.alertTopic = new Topic(this, 'AlertTopic', {
				displayName: `robocode-s rumble ${props.stage}`
			});
			this.alertTopic.addSubscription(new EmailSubscription(props.alertEmail));
		}
		const alarm = (name: string, metric: Metric, threshold: number, description: string) => {
			const alarm = new Alarm(this, name, {
				metric,
				threshold,
				evaluationPeriods: 1,
				comparisonOperator: ComparisonOperator.GREATER_THAN_THRESHOLD,
				treatMissingData: TreatMissingData.NOT_BREACHING,
				alarmDescription: description
			});
			if (this.alertTopic) alarm.addAlarmAction(new SnsAction(this.alertTopic));
		};
		for (const [name, group] of [
			['Api', apiLogs],
			['Batch', batchLogs]
		] as const) {
			const filter = new MetricFilter(this, `${name}AlertFilter`, {
				logGroup: group,
				filterPattern: FilterPattern.anyTerm('ALERT'),
				metricNamespace: 'RobocodeS/Rumble',
				metricName: `${props.stage}${name}Alerts`,
				metricValue: '1',
				defaultValue: 0
			});
			alarm(
				`${name}AlertAlarm`,
				filter.metric({ period: Duration.minutes(5), statistic: 'Sum' }),
				0,
				`ALERT lines in the ${name.toLowerCase()} Lambda's log`
			);
		}
		alarm(
			'ApiThrottleAlarm',
			this.api.metricThrottles({ period: Duration.minutes(15), statistic: 'Sum' }),
			0,
			'The api Lambda is throttled (reserved concurrency 10)'
		);
		alarm(
			'TableWriteThrottleAlarm',
			this.table.metric('WriteThrottleEvents', { period: Duration.minutes(15), statistic: 'Sum' }),
			100,
			'DynamoDB write throttling'
		);

		new CfnOutput(this, 'ApiFunctionUrl', {
			value: url.url,
			description: 'For debugging only: it refuses unsigned calls'
		});
		new CfnOutput(this, 'JarsBucketName', { value: this.jarsBucket.bucketName });
		new CfnOutput(this, 'StateBucketName', { value: this.stateBucket.bucketName });
		new CfnOutput(this, 'TableName', { value: this.table.tableName });
	}
}
