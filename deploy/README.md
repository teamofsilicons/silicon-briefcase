# Deploying Silicon Briefcase

The canonical [deployment runbook](../docs/deployment.md) now lives under the
backend `docs/` directory alongside the [API/client/CLI guides](../docs/README.md).

Deployment scripts, the CloudFormation template, and example configuration
remain in this directory. Run the documented commands from the repository root.

## Replacement safety

The deploy script uses launch-before-terminate refreshes: a 100% minimum healthy
capacity and an explicit 200% maximum allow the singleton group to temporarily
run its replacement alongside the existing instance. The old instance stays
available until the replacement passes its health checks and the 600-second
warmup, avoiding a planned zero-capacity interval. See AWS's
[instance refresh behavior](https://docs.aws.amazon.com/autoscaling/ec2/userguide/instance-refresh-overview.html)
and [refresh preferences](https://docs.aws.amazon.com/autoscaling/ec2/APIReference/API_RefreshPreferences.html).

The existing 900-second load-balancer deregistration delay remains unchanged so
in-flight uploads can finish before the old instance terminates. Allow time for
bootstrapping, warmup, and draining; do not shorten draining to speed up a deploy.
Instances already matching the launch-template version are skipped. Each new
immutable image must therefore produce a new launch-template version; rerunning
`refresh` without a configuration change does not force a replacement.
