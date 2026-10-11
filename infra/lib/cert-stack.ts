// TLS certificate for the site. CloudFront only accepts certificates from us-east-1, so this stack
// lives there and hands the certificate to the Site stack through a cross-region reference.
// The DNS zone is in the Organization's management account (ADR 0016), so validation is manual:
// the stack waits until the validation CNAME is added there (infra/README.md).

import { CfnOutput, Stack, type StackProps } from 'aws-cdk-lib';
import {
	Certificate,
	CertificateValidation,
	type ICertificate
} from 'aws-cdk-lib/aws-certificatemanager';
import type { Construct } from 'constructs';

export interface CertStackProps extends StackProps {
	domain: string;
}

export class CertStack extends Stack {
	readonly certificate: ICertificate;

	constructor(scope: Construct, id: string, props: CertStackProps) {
		super(scope, id, { ...props, crossRegionReferences: true });
		this.certificate = new Certificate(this, 'Certificate', {
			domainName: props.domain,
			validation: CertificateValidation.fromDns()
		});
		new CfnOutput(this, 'CertificateArn', { value: this.certificate.certificateArn });
	}
}
