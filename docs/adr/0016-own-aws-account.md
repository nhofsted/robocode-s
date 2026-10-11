# 0016. The rumble runs in its own AWS account

- Status: Accepted
- Date: 2026-10-12

## Context

The owner's AWS Organization has one account, the management account, which runs another project's production workloads and hosts the `planethofstede.be` zone. The rumble adds a public repository with a GitHub deploy role and a public API for volunteers (ADR 0007). The free-tier allowances are counted once per Organization, so a second account brings no extra free tier; the other project uses only a few percent of it.

## Decision

- The rumble runs in a **member account `robocode-s`**, created and configured by `infra/account/setup.sh`. CLI profile `robocode-s`, through the existing Identity Center sign-in.
- An **SCP** (`infra/account/guardrails-scp.json`) limits the account to eu-west-1 and us-east-1 and denies services that cost money regardless of use (EC2 instances, NAT gateways, databases other than DynamoDB, load balancers, provisioned Lambda concurrency, Secrets Manager, hosted zones). A $1/month budget alerts the owner.
- **DNS stays in the management account.** The `rumble` record and the certificate's validation record are added to `planethofstede.be` by hand, once. No second hosted zone ($0.50/month), no cross-account role.
- **DynamoDB:** provisioned at the free limits in prod, so overload throttles instead of billing. A dev stage uses on-demand: its traffic costs cents and it leaves the shared free capacity to prod.

## Consequences

- A deploy role or Lambda bug in the rumble can't reach the other project's data.
- The SCP enforces the free-tier rule in IAM instead of relying on review. A new service the rumble needs may need an SCP change.
- The two DNS records live outside CDK. CDK prints the values; they change only when the distribution or certificate is replaced.
