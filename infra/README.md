# infra: AWS for the robocode-s rumble

AWS CDK (TypeScript) for the rumble (design §12, ADR 0007), in its own AWS account (ADR 0016).

| Stack | Region | Contents |
|---|---|---|
| `RobocodeS-Cert` | us-east-1 | ACM certificate for `rumble.planethofstede.be` (CloudFront needs us-east-1) |
| `RobocodeS-Site` | eu-west-1 | S3 bucket, CloudFront distribution, the page-request function, the site and the sample data; the packages bucket (`packages/*`: the apt and dnf repositories, filled by `tools/release/repo.py`, not by CDK); the rumble backend ([`lib/rumble.ts`](lib/rumble.ts), below) |
| `RobocodeS-Dev-Site` | eu-west-1 | The same stack for `-c stage=dev`: no domain, no certificate (`dxxxx.cloudfront.net`), on-demand table, its own buckets, sample data kept. Deployed instead of the two above |

The account itself (member account, Identity Center access, guardrail SCP, budget, CDK bootstrap) is set up once by [`account/setup.sh`](account/setup.sh).

## Deploying

Requires Node 24 and pnpm, and the `robocode-s` profile signed in (`aws sso login --profile robocode-s`).

```sh
pnpm install
pnpm test         # the page-request function and the rumble construct
pnpm typecheck
pnpm diff         # builds nothing: run pnpm build:site and pnpm build:lambdas first
pnpm deploy       # builds web/, the sample data and the Lambdas, deploys both prod stacks
pnpm deploy:dev   # the same for RobocodeS-Dev-Site (-c stage=dev)
```

Context values (`-c name=value`; `cdk.json` is committed, so personal values such as `alertEmail` go on the command line):

| Context | Needed | Meaning |
|---|---|---|
| `stage` | no | `prod` (default) or `dev` |
| `githubClientId` / `githubClientIdDev` | prod / dev | Client id of the stage's GitHub OAuth app (not a secret) |
| `alertEmail` | no | Where the alarms are mailed. Without it there is no SNS topic and the alarms only show in the console. Confirm the subscription e-mail AWS sends |
| `devOrigin` | dev, second deploy | `https://dxxxx.cloudfront.net`, see the dev stage below |
| `sampleData` | no | `true` deploys the synthetic sample data under `data/v1/`. Default: `true` in dev, `false` in prod, because its deployment prunes `data/v1/` and would delete the files `batch publish` wrote. The files already in the bucket stay when it is turned off |

`pnpm build:lambdas` ([`tools/rumble/build-lambdas.sh`](../tools/rumble/build-lambdas.sh): cargo-zigbuild, arm64) writes `rumble/lambda/target/lambda/{api,batch}/bootstrap`, which the synth needs. The tests use a placeholder instead.

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

## Package repositories

`https://rumble.planethofstede.be/packages/` is a second bucket (`PackagesBucketName` output; kept when the stack is deleted; versioned) behind a `packages/*` behavior of the same distribution: no new DNS record, no new certificate. The behavior has no page-request function (apt's `InRelease` has no extension) and no compression. `tools/release/repo.py upload` (design §6.9) puts the objects under the `packages/` prefix and reads `PackagesBucketName` and `DistributionId` from this stack's outputs. The first `pnpm deploy` after this change adds the bucket and the behavior; the repositories stay empty until the owner runs `release.sh publish`.

## The rumble backend

`lib/rumble.ts` (design: [`docs/rumble-backend.md`](../docs/rumble-backend.md) §8, §9, §14):

- **Table** `rumble` (prod: provisioned 25/25, no autoscaling, deletion protection, retained) or `rumble-dev` (on demand, destroyed with the stack); `pk`/`sk`, TTL `ttl`, no GSIs.
- **Jars bucket** (`/jars/*`, `/rec/*` through CloudFront OAC, retained in prod) and **state bucket** (private, versioned; `uploads/` expires after 1 day, old versions after 7; CORS allows `PUT` for browser uploads of jars from the site's origin).
- **Lambdas** `api` (256 MB, 25 s, reserved concurrency 10) behind a function URL (`AWS_IAM`, buffered) that CloudFront reaches with an origin access control on `/api/*`, and `batch` (1,024 MB, 900 s, reserved concurrency 1). Logs are kept 7 days. The origin request policy for `/api/*` lists the headers the API reads (not `Host`; it adds `CloudFront-Viewer-Address`), since the managed `AllViewerExceptHostHeader` policy cannot be extended. CDK's OAC grants `cloudfront.amazonaws.com` both `lambda:InvokeFunctionUrl` and `lambda:InvokeFunction` for the distribution (the tests check it).
- **Schedules** (EventBridge Scheduler, UTC) invoking `batch` with `{"mode": ...}`: `plan` hourly at :05, `publish` at 03:30, 09:30, 15:30 and 21:30, `cleanup` daily at 04:45.
- **IAM**: each Lambda gets exactly what is listed in `rumble.ts` (`allow(fn, actions, resources)`: add a line when the code needs more); SSM read on `/robocode-s/<stage>/*` with `kms:Decrypt` only through SSM.
- **Alarms**: `ALERT` lines in either Lambda's log, `api` throttles, table write throttling, mailed through SNS if `alertEmail` is set.

**Reserved concurrency needs the account's Lambda concurrency quota above 10 (it must leave 10 unreserved).** New accounts often have 10: request 1,000 through Service Quotas (free) before the first deploy.

### Secrets and the GitHub OAuth app

One GitHub OAuth app per stage (Settings, Developer settings, OAuth Apps). Callback URL `https://<domain>/api/v1/auth/callback` (prod: `rumble.planethofstede.be`; dev: the `dxxxx.cloudfront.net` domain, known after the first deploy; GitHub accepts a changed callback later). Tick "Enable Device Flow". The client id goes into `githubClientId` / `githubClientIdDev`; the secret into SSM:

```sh
tools/rumble/secrets.sh dev    # or prod: creates the signing key if absent, prompts for the client secret
```

The Lambdas read both SecureStrings at cold start; CDK never sees their values. Run it before the first request, not necessarily before the deploy.

### The dev stage and its origin

Dev has no domain, so its URL is only known after the first deploy, and the `api` Lambda needs it (`SITE_ORIGIN`: CSRF check and redirects) while the distribution points at the Lambda (the distribution cannot be read from the Lambda's environment: a dependency cycle). So dev deploys twice, and the second is a one-off:

1. Create the OAuth app with any callback URL, then `pnpm deploy:dev -c githubClientIdDev=<id>`. `SITE_ORIGIN` is a placeholder (`*.invalid`); the state bucket allows browser `PUT`s from `https://*.cloudfront.net`.
2. Take `DistributionDomainName` from the outputs, set the OAuth app's callback to `https://<it>/api/v1/auth/callback`, run `tools/rumble/secrets.sh dev`.
3. `pnpm deploy:dev -c githubClientIdDev=<id> -c devOrigin=https://<it>` (keep passing both from then on; the domain only changes if the distribution is replaced).
