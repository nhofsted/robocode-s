// The static site (web/, ADR 0015) on S3 + CloudFront, at the rumble's domain (design §12.1).
// Later the rumble's Lambdas join this distribution as /api/* origins, and the rankings Lambda
// writes data/v1/ into the bucket. Until then the bucket holds the synthetic sample data.

import { fileURLToPath } from 'node:url';
import { CfnOutput, Duration, RemovalPolicy, Size, Stack, type StackProps } from 'aws-cdk-lib';
import type { ICertificate } from 'aws-cdk-lib/aws-certificatemanager';
import {
	CachePolicy,
	Distribution,
	FunctionCode,
	FunctionEventType,
	FunctionRuntime,
	Function as CloudFrontFunction,
	HttpVersion,
	PriceClass,
	ResponseHeadersPolicy,
	ViewerProtocolPolicy
} from 'aws-cdk-lib/aws-cloudfront';
import { S3BucketOrigin } from 'aws-cdk-lib/aws-cloudfront-origins';
import { BlockPublicAccess, Bucket, BucketEncryption } from 'aws-cdk-lib/aws-s3';
import { BucketDeployment, CacheControl, Source } from 'aws-cdk-lib/aws-s3-deployment';
import type { Construct } from 'constructs';

export interface SiteStackProps extends StackProps {
	domain: string;
	certificate: ICertificate;
	/** `pnpm build` output of web/. */
	siteDir: string;
	/** Files to publish under data/v1/ (web/tools/sample-data.ts), until the rankings Lambda runs. */
	sampleDataDir?: string;
}

const repoPath = (path: string) => fileURLToPath(new URL(`../../${path}`, import.meta.url));

export class SiteStack extends Stack {
	constructor(scope: Construct, id: string, props: SiteStackProps) {
		super(scope, id, { ...props, crossRegionReferences: true });

		const bucket = new Bucket(this, 'SiteBucket', {
			blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
			encryption: BucketEncryption.S3_MANAGED,
			enforceSSL: true
		});

		const pageRequests = new CloudFrontFunction(this, 'PageRequests', {
			runtime: FunctionRuntime.JS_2_0,
			comment: 'Pretty URLs and data pages from 200.html (infra/lib/page-requests.js)',
			code: FunctionCode.fromFile({ filePath: repoPath('infra/lib/page-requests.js') })
		});

		// The apt and dnf repositories (design §6.9), under /packages/ of the same domain: no new
		// DNS record or certificate. tools/release/repo.py uploads into packages/ of this bucket.
		// Not deployed from CDK: the packages and their signed metadata come from the owner's machine.
		const packagesBucket = new Bucket(this, 'PackagesBucket', {
			blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
			encryption: BucketEncryption.S3_MANAGED,
			enforceSSL: true,
			removalPolicy: RemovalPolicy.RETAIN,
			versioned: true,
			lifecycleRules: [{ noncurrentVersionExpiration: Duration.days(30) }]
		});

		// Caching follows the objects' Cache-Control, set per deployment below.
		const distribution = new Distribution(this, 'Distribution', {
			comment: 'robocode-s site',
			domainNames: [props.domain],
			certificate: props.certificate,
			priceClass: PriceClass.PRICE_CLASS_100,
			httpVersion: HttpVersion.HTTP2_AND_3,
			defaultBehavior: {
				origin: S3BucketOrigin.withOriginAccessControl(bucket),
				viewerProtocolPolicy: ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
				cachePolicy: CachePolicy.CACHING_OPTIMIZED,
				responseHeadersPolicy: ResponseHeadersPolicy.SECURITY_HEADERS,
				compress: true,
				functionAssociations: [
					{ function: pageRequests, eventType: FunctionEventType.VIEWER_REQUEST }
				]
			},
			additionalBehaviors: {
				// No page-request function: apt's `InRelease` and the like have no extension and must
				// not become `.html`. Caching follows the objects' Cache-Control (index files 5 minutes,
				// packages immutable); no compression, the .gz and .deb files are already compressed.
				'packages/*': {
					origin: S3BucketOrigin.withOriginAccessControl(packagesBucket),
					viewerProtocolPolicy: ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
					cachePolicy: CachePolicy.CACHING_OPTIMIZED,
					responseHeadersPolicy: ResponseHeadersPolicy.SECURITY_HEADERS,
					compress: false
				}
			},
			// S3 answers 403 for missing objects (no list permission). Unknown pages render the
			// site's error page from 200.html; the status stays 404, which the data loader expects.
			errorResponses: [403, 404].map((httpStatus) => ({
				httpStatus,
				responseHttpStatus: 404,
				responsePagePath: '/200.html',
				ttl: Duration.minutes(1)
			}))
		});

		// Hashed assets: kept across deploys, so pages cached before a deploy still load theirs.
		const immutable = new BucketDeployment(this, 'DeployImmutable', {
			destinationBucket: bucket,
			destinationKeyPrefix: '_app/immutable/',
			sources: [Source.asset(`${props.siteDir}/_app/immutable`)],
			prune: false,
			cacheControl: [CacheControl.fromString('public, max-age=31536000, immutable')]
		});

		const data =
			props.sampleDataDir &&
			new BucketDeployment(this, 'DeploySampleData', {
				destinationBucket: bucket,
				destinationKeyPrefix: 'data/v1/',
				sources: [Source.asset(props.sampleDataDir)],
				cacheControl: [CacheControl.fromString('public, max-age=3600')],
				// About 150 MB of JSON, unzipped in the handler.
				memoryLimit: 2048,
				ephemeralStorageSize: Size.gibibytes(2)
			});

		// Pages last, then one invalidation for everything.
		const pages = new BucketDeployment(this, 'DeployPages', {
			destinationBucket: bucket,
			sources: [Source.asset(props.siteDir, { exclude: ['_app/immutable/**'] })],
			exclude: ['_app/immutable/*', 'data/*'],
			cacheControl: [CacheControl.fromString('public, max-age=300')],
			distribution,
			distributionPaths: ['/*']
		});
		pages.node.addDependency(immutable);
		if (data) pages.node.addDependency(data);

		new CfnOutput(this, 'DistributionDomainName', {
			value: distribution.distributionDomainName,
			description: `CNAME target for ${props.domain}`
		});
		new CfnOutput(this, 'BucketName', { value: bucket.bucketName });
		new CfnOutput(this, 'PackagesBucketName', {
			value: packagesBucket.bucketName,
			description: 'tools/release/repo.py upload: s3://<this>/packages/'
		});
		new CfnOutput(this, 'DistributionId', {
			value: distribution.distributionId,
			description: 'tools/release/repo.py upload: the invalidation'
		});
	}
}
