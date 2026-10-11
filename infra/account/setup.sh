#!/usr/bin/env bash
# One-time setup of the robocode-s AWS account (ADR 0016). Run by the owner, signed in to the
# Organization's management account through Identity Center. Safe to re-run: every step checks
# what already exists.
#
#   aws sso login --profile nodaly
#   ACCOUNT_EMAIL=you+robocode-s@example.com infra/account/setup.sh
#
# ACCOUNT_EMAIL becomes the new account's root email and receives the budget alerts.
#
# It creates the robocode-s member account, assigns AdministratorAccess to the Identity Center
# user, attaches the guardrail SCP, adds the robocode-s CLI profile, creates a $1/month budget
# and bootstraps CDK in eu-west-1 and us-east-1.
set -euo pipefail

MGMT_PROFILE=${MGMT_PROFILE:-nodaly}
PROFILE=robocode-s
ACCOUNT_NAME=robocode-s
ACCOUNT_EMAIL=${ACCOUNT_EMAIL:?set ACCOUNT_EMAIL to the root email of the new account}
PERMISSION_SET=AdministratorAccess
SCP_NAME=robocode-s-guardrails
CDK_VERSION=2.1143.0
HERE=$(cd "$(dirname "$0")" && pwd)

mgmt() { aws --profile "$MGMT_PROFILE" "$@"; }
step() { printf '\n== %s\n' "$*"; }

step "Account"
ACCOUNT_ID=$(mgmt organizations list-accounts \
	--query "Accounts[?Name=='$ACCOUNT_NAME' && Status=='ACTIVE'].Id | [0]" --output text)
if [[ $ACCOUNT_ID == None ]]; then
	REQUEST=$(mgmt organizations create-account --email "$ACCOUNT_EMAIL" --account-name "$ACCOUNT_NAME" \
		--iam-user-access-to-billing ALLOW --query CreateAccountStatus.Id --output text)
	while :; do
		read -r STATE ACCOUNT_ID REASON < <(mgmt organizations describe-create-account-status \
			--create-account-request-id "$REQUEST" \
			--query 'CreateAccountStatus.[State,AccountId,FailureReason]' --output text)
		[[ $STATE == IN_PROGRESS ]] || break
		sleep 10
	done
	[[ $STATE == SUCCEEDED ]] || { echo "account creation $STATE: $REASON" >&2; exit 1; }
fi
echo "$ACCOUNT_NAME: $ACCOUNT_ID"

step "Identity Center: $PERMISSION_SET"
read -r INSTANCE STORE < <(mgmt sso-admin list-instances \
	--query 'Instances[0].[InstanceArn,IdentityStoreId]' --output text)
PS_ARN=""
for arn in $(mgmt sso-admin list-permission-sets --instance-arn "$INSTANCE" --query PermissionSets --output text); do
	name=$(mgmt sso-admin describe-permission-set --instance-arn "$INSTANCE" --permission-set-arn "$arn" \
		--query PermissionSet.Name --output text)
	[[ $name == "$PERMISSION_SET" ]] && PS_ARN=$arn
done
[[ -n $PS_ARN ]] || { echo "no permission set $PERMISSION_SET" >&2; exit 1; }
# The management profile's role session name is the Identity Center user name.
USER_NAME=${SSO_USER:-$(mgmt sts get-caller-identity --query UserId --output text | cut -d: -f2)}
USER_ID=$(mgmt identitystore get-user-id --identity-store-id "$STORE" \
	--alternate-identifier "{\"UniqueAttribute\":{\"AttributePath\":\"userName\",\"AttributeValue\":\"$USER_NAME\"}}" \
	--query UserId --output text)
ASSIGNED=$(mgmt sso-admin list-account-assignments --instance-arn "$INSTANCE" --account-id "$ACCOUNT_ID" \
	--permission-set-arn "$PS_ARN" --query "AccountAssignments[?PrincipalId=='$USER_ID'] | length(@)" --output text)
if [[ $ASSIGNED == 0 ]]; then
	REQUEST=$(mgmt sso-admin create-account-assignment --instance-arn "$INSTANCE" \
		--target-id "$ACCOUNT_ID" --target-type AWS_ACCOUNT --permission-set-arn "$PS_ARN" \
		--principal-type USER --principal-id "$USER_ID" \
		--query AccountAssignmentCreationStatus.RequestId --output text)
	while :; do
		STATE=$(mgmt sso-admin describe-account-assignment-creation-status --instance-arn "$INSTANCE" \
			--account-assignment-creation-request-id "$REQUEST" \
			--query AccountAssignmentCreationStatus.Status --output text)
		[[ $STATE == IN_PROGRESS ]] || break
		sleep 5
	done
	[[ $STATE == SUCCEEDED ]] || { echo "account assignment $STATE" >&2; exit 1; }
fi
echo "$USER_NAME has $PERMISSION_SET"

step "SCP $SCP_NAME"
POLICY_ID=$(mgmt organizations list-policies --filter SERVICE_CONTROL_POLICY \
	--query "Policies[?Name=='$SCP_NAME'].Id | [0]" --output text)
if [[ $POLICY_ID == None ]]; then
	POLICY_ID=$(mgmt organizations create-policy --type SERVICE_CONTROL_POLICY --name "$SCP_NAME" \
		--description "robocode-s: two regions, no services outside the free-tier plan (ADR 0016)" \
		--content "file://$HERE/guardrails-scp.json" --query Policy.PolicySummary.Id --output text)
else
	mgmt organizations update-policy --policy-id "$POLICY_ID" \
		--content "file://$HERE/guardrails-scp.json" >/dev/null
fi
if ! mgmt organizations list-targets-for-policy --policy-id "$POLICY_ID" \
	--query 'Targets[].TargetId' --output text | grep -qw "$ACCOUNT_ID"; then
	mgmt organizations attach-policy --policy-id "$POLICY_ID" --target-id "$ACCOUNT_ID"
fi
echo "$SCP_NAME ($POLICY_ID) attached"

step "CLI profile $PROFILE"
SSO_SESSION=$(aws configure get sso_session --profile "$MGMT_PROFILE")
if ! aws configure list-profiles | grep -qx "$PROFILE"; then
	cat >>"${AWS_CONFIG_FILE:-$HOME/.aws/config}" <<-EOF
	[profile $PROFILE]
	sso_session = $SSO_SESSION
	sso_account_id = $ACCOUNT_ID
	sso_role_name = $PERMISSION_SET
	region = eu-west-1
	output = json
	EOF
fi
# A fresh assignment takes a moment to reach the account.
for _ in {1..30}; do
	aws sts get-caller-identity --profile "$PROFILE" --query Arn --output text 2>/dev/null && break
	sleep 10
done

step "Budget"
if ! aws budgets describe-budget --profile "$PROFILE" --account-id "$ACCOUNT_ID" \
	--budget-name robocode-s-monthly >/dev/null 2>&1; then
	aws budgets create-budget --profile "$PROFILE" --account-id "$ACCOUNT_ID" \
		--budget '{"BudgetName":"robocode-s-monthly","BudgetType":"COST","TimeUnit":"MONTHLY","BudgetLimit":{"Amount":"1","Unit":"USD"}}' \
		--notifications-with-subscribers "[
			{\"Notification\":{\"NotificationType\":\"ACTUAL\",\"ComparisonOperator\":\"GREATER_THAN\",\"Threshold\":50,\"ThresholdType\":\"PERCENTAGE\"},\"Subscribers\":[{\"SubscriptionType\":\"EMAIL\",\"Address\":\"$ACCOUNT_EMAIL\"}]},
			{\"Notification\":{\"NotificationType\":\"FORECASTED\",\"ComparisonOperator\":\"GREATER_THAN\",\"Threshold\":100,\"ThresholdType\":\"PERCENTAGE\"},\"Subscribers\":[{\"SubscriptionType\":\"EMAIL\",\"Address\":\"$ACCOUNT_EMAIL\"}]}
		]"
fi
echo "robocode-s-monthly: \$1, alerts to $ACCOUNT_EMAIL"

step "CDK bootstrap"
npx -y "aws-cdk@$CDK_VERSION" bootstrap --profile "$PROFILE" \
	"aws://$ACCOUNT_ID/eu-west-1" "aws://$ACCOUNT_ID/us-east-1"

step "Lambda concurrency quota"
# New accounts can start far below the default 1000; reserved concurrency per function needs headroom.
aws service-quotas get-service-quota --profile "$PROFILE" --service-code lambda --quota-code L-B99A9384 \
	--query Quota.Value --output text

printf '\nDone. Account %s, profile %s.\n' "$ACCOUNT_ID" "$PROFILE"
