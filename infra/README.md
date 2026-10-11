# infra: AWS for the robocode-s rumble

AWS CDK (TypeScript) for the rumble (design §12, ADR 0007), in its own AWS account (ADR 0016).

| Stack | Region | Contents |
|---|---|---|
| `RobocodeS-Cert` | us-east-1 | ACM certificate for `rumble.planethofstede.be` (CloudFront needs us-east-1) |
| `RobocodeS-Site` | eu-west-1 | S3 bucket, CloudFront distribution, the page-request function, the site and the sample data |

The account itself (member account, Identity Center access, guardrail SCP, budget, CDK bootstrap) is set up once by [`account/setup.sh`](account/setup.sh).

## Deploying

Requires Node 24 and pnpm, and the `robocode-s` profile signed in (`aws sso login --profile robocode-s`).

```sh
pnpm install
pnpm test         # the page-request function
pnpm typecheck
pnpm diff         # builds nothing: run pnpm build:site first
pnpm deploy       # builds web/ and the sample data, deploys both stacks
```

`pnpm build:site` writes `web/build` and `.sample-data/` (`web/tools/sample-data.ts`). Without `.sample-data/` the site deploys without data files and keeps those already in the bucket.

## DNS

The `planethofstede.be` zone stays in the management account (ADR 0016), so two records are added there by hand. Both only change when the certificate or the distribution is replaced.

1. **Certificate validation.** The first deploy of `RobocodeS-Cert` waits until the certificate is validated. Get the record while it waits and add it to the zone:
   ```sh
   aws acm describe-certificate --profile robocode-s --region us-east-1 \
     --certificate-arn "$(aws acm list-certificates --profile robocode-s --region us-east-1 \
       --query "CertificateSummaryList[?DomainName=='rumble.planethofstede.be'].CertificateArn | [0]" --output text)" \
     --query 'Certificate.DomainValidationOptions[0].ResourceRecord'
   ```
2. **The site.** A CNAME `rumble.planethofstede.be` → the `DistributionDomainName` output of `RobocodeS-Site` (`dxxxx.cloudfront.net`).

## Caching

CloudFront follows the objects' `Cache-Control`: hashed assets under `_app/immutable/` for a year (kept across deploys), pages for 5 minutes, data files for an hour. Every deploy invalidates `/*`. Unknown paths get `200.html` with status 404, so the site shows its error page and data requests keep their 404.
