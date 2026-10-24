#!/usr/bin/env bash
# SPDX-License-Identifier: EPL-2.0
#
# Creates the rumble's SSM parameters for a stage (docs/rumble-backend.md §14.5), once:
#   /robocode-s/<stage>/signing-key          SecureString "k1:<64 hex>", only if absent
#   /robocode-s/<stage>/github-client-secret SecureString, prompted (the GitHub OAuth app's secret)
#
#   tools/rumble/secrets.sh dev|prod
set -euo pipefail

stage="${1:-}"
case "$stage" in
  dev|prod) ;;
  *) echo "usage: $0 dev|prod" >&2; exit 2 ;;
esac

aws=(aws --profile robocode-s --region eu-west-1)
prefix="/robocode-s/$stage"

if "${aws[@]}" ssm get-parameter --name "$prefix/signing-key" >/dev/null 2>&1; then
  echo "$prefix/signing-key exists, kept"
else
  key="k1:$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  "${aws[@]}" ssm put-parameter --name "$prefix/signing-key" --type SecureString --value "$key" >/dev/null
  echo "$prefix/signing-key created"
fi

read -r -s -p "GitHub OAuth client secret for $stage (empty keeps the current one): " secret
echo
if [ -n "$secret" ]; then
  # A GitHub OAuth app's client secret is 40 hex digits: catch a paste that added to it.
  if ! [[ "$secret" =~ ^[0-9a-f]{40}$ ]]; then
    echo "that is ${#secret} characters, not the 40 hex digits of a client secret: not stored" >&2
    exit 1
  fi
  "${aws[@]}" ssm put-parameter --name "$prefix/github-client-secret" --type SecureString \
    --value "$secret" --overwrite >/dev/null
  echo "$prefix/github-client-secret stored"
else
  echo "$prefix/github-client-secret unchanged"
fi
