#!/usr/bin/env python3
"""Backfills Content-Disposition on the rumble's stored jars (one-off, rerunnable).

Jars are stored as jars/<sha256>.jar. Since the Lambdas set `Content-Disposition: attachment;
filename="<classname>_<version>.jar"` on new jars, this gives the jars stored before that the
same header: each jar is copied onto itself with the new metadata. A jar shared by several
versions gets the name of the lowest vid (the first registration, as the Lambdas do).

Usage (AWS_PROFILE=robocode-s, aws CLI v2 on PATH):
    tools/rumble/jar-names.py --stage prod            # dry run: counts and examples
    tools/rumble/jar-names.py --stage prod --apply    # copy, then invalidate /jars/* on the CDN

The version query reads the REG partition paced by --page-size, so it doesn't starve the
table's provisioned reads; run it away from the batch (:12 plan, :30 publish).
"""
import argparse
import json
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

REGION = 'eu-west-1'
JAR_TYPE = 'application/java-archive'
IMMUTABLE = 'public, max-age=31536000, immutable'
STACKS = {'prod': ('RobocodeS-Site', 'rumble'), 'dev': ('RobocodeS-Dev-Site', 'rumble-dev')}


def aws(*args):
    out = subprocess.run(['aws', '--region', REGION, '--output', 'json', *args],
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out) if out.strip() else None


def jar_file_name(bot_id, version):
    """As s3::jar_file_name in the Lambda: unsafe characters become `_`."""
    safe = lambda s: re.sub(r'[^A-Za-z0-9._$+-]', '_', s)
    return f'{safe(bot_id)}_{safe(version)}.jar'


def stack_output(stack, part):
    outputs = aws('cloudformation', 'describe-stacks', '--stack-name', stack,
                  '--query', 'Stacks[0].Outputs')
    return next(o['OutputValue'] for o in outputs if part in o['OutputKey'])


def names_by_sha(table, page_size):
    items = aws('dynamodb', 'query', '--table-name', table,
                '--key-condition-expression', 'pk = :p AND begins_with(sk, :v)',
                '--expression-attribute-values', '{":p":{"S":"REG"},":v":{"S":"V#"}}',
                '--projection-expression', 'vid,bot,ver,sha', '--page-size', str(page_size))['Items']
    first = {}
    for it in items:
        vid, sha = int(it['vid']['N']), it['sha']['S']
        if sha not in first or vid < first[sha][0]:
            first[sha] = (vid, jar_file_name(it['bot']['S'], it['ver']['S']))
    return len(items), {sha: name for sha, (_, name) in first.items()}


def list_jars(bucket):
    keys, token = [], None
    while True:
        args = ['s3api', 'list-objects-v2', '--bucket', bucket, '--prefix', 'jars/']
        if token:
            args += ['--continuation-token', token]
        page = aws(*args)
        keys += [o['Key'] for o in page.get('Contents', [])]
        token = page.get('NextContinuationToken')
        if not token:
            return keys


def set_name(bucket, key, name):
    aws('s3api', 'copy-object', '--bucket', bucket, '--key', key,
        '--copy-source', f'{bucket}/{key}', '--metadata-directive', 'REPLACE',
        '--content-type', JAR_TYPE, '--cache-control', IMMUTABLE,
        '--content-disposition', f'attachment; filename="{name}"',
        '--checksum-algorithm', 'SHA256')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--stage', choices=STACKS, required=True)
    ap.add_argument('--apply', action='store_true', help='copy (default: dry run)')
    ap.add_argument('--page-size', type=int, default=200)
    ap.add_argument('--jobs', type=int, default=8)
    a = ap.parse_args()
    stack, table = STACKS[a.stage]
    bucket = stack_output(stack, 'RumbleJarsBucket')
    versions, names = names_by_sha(table, a.page_size)
    keys = list_jars(bucket)
    todo = [(k, names[k[5:-4]]) for k in keys if k[5:-4] in names]
    orphans = [k for k in keys if k[5:-4] not in names]
    print(f'{versions} versions, {len(names)} distinct jars named; bucket {bucket} holds '
          f'{len(keys)} jars: {len(todo)} to name, {len(orphans)} without a version (left alone)')
    for key, name in todo[:5]:
        print(f'  {key} -> {name}')
    if not a.apply:
        print('dry run: rerun with --apply')
        return
    failed = []

    def one(job):
        try:
            set_name(bucket, *job)
        except subprocess.CalledProcessError as e:
            failed.append((job[0], e.stderr.strip()))

    with ThreadPoolExecutor(a.jobs) as pool:
        for n, _ in enumerate(pool.map(one, todo), 1):
            if n % 500 == 0:
                print(f'  {n}/{len(todo)}', flush=True)
    print(f'named {len(todo) - len(failed)}, failed {len(failed)}')
    for key, err in failed[:10]:
        print(f'  {key}: {err}')
    distribution = next(o for o in aws('cloudfront', 'list-distributions')['DistributionList']['Items']
                        if any(bucket in x['DomainName'] for x in o['Origins']['Items']))
    inv = aws('cloudfront', 'create-invalidation', '--distribution-id', distribution['Id'],
              '--paths', '/jars/*')
    print('CDN invalidation', inv['Invalidation']['Id'], 'for /jars/*')
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
