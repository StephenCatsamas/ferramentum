# ice

Minimal CLI for deploying and managing workloads on `vast.ai`, `gcp`, `aws`, and `local`.

## Install

```bash
cargo install ice-tool
```

Installed command: `ice`

## Quick start

```bash
ice login --cloud vast.ai
ice create test-crate
ice list --cloud vast.ai
ice logs --cloud vast.ai <instance> --follow
ice delete --cloud vast.ai <instance>
```

## Clouds

Supported cloud identifiers:

- `vast.ai`
- `gcp`
- `aws`
- `local`

## Commands

- `ice login [--cloud CLOUD] [--force]`
- `ice config list`
- `ice config get <KEY>`
- `ice config set <KEY=VALUE>`
- `ice config unset <KEY>`
- `ice list [--cloud CLOUD]`
- `ice logs [--cloud CLOUD] <INSTANCE> [--tail N] [--follow]`
- `ice shell [--cloud CLOUD] <INSTANCE>`
- `ice pull [--cloud CLOUD] <INSTANCE> <REMOTE_PATH> [LOCAL_PATH]`
- `ice push [--cloud CLOUD] <INSTANCE> <LOCAL_PATH> [REMOTE_PATH]`
- `ice stop [--cloud CLOUD] <INSTANCE>`
- `ice start [--cloud CLOUD] <INSTANCE>`
- `ice delete [--cloud CLOUD] <INSTANCE>`
- `ice create [--cloud CLOUD] [--hours HOURS] [--machine MACHINE] [--custom] [--dry-run] [--ssh | --container IMAGE_REF | --unpack SOURCE | --arca [ARTIFACT] | TARGET]`

`<INSTANCE>` accepts an instance id or label.

## JSON output

Add `--json` to these existing commands:

```sh
ice list --cloud vast.ai --json
ice create --cloud vast.ai --ssh --dry-run --json
ice create --cloud vast.ai --ssh --json
ice shell --cloud vast.ai INSTANCE --print-creds --json
```

Listing supports all providers. Creation previews support all providers and target
modes; actual JSON creation supports `--ssh` machines on Vast, GCP and AWS. Managed
workload creation can produce arbitrary output and currently rejects `--json`
unless `--dry-run` is supplied. Connection output supports Vast, GCP and AWS;
`shell --json` requires `--print-creds`.

Successful commands write exactly one JSON object and a newline to stdout:

```json
{"schema_version":1,"command":"list","cloud":"vast.ai","result":{"instances":[]}}
```

- `list` returns an `instances` array with stable string IDs, names, provider state
  and provider-specific fields such as zone, GPU model or SSH endpoint. Missing
  values are `null`; records omit display colors and raw provider metadata.
- `create` returns a `status` of `preview`, `created` or `cancelled`, along with
  the selected offer/machine and cost estimate when applicable. A created result
  includes the instance ID. Vast results include allocated disk, the requested
  image reference and the scheduled UTC stop time as Unix seconds. An automatic
  image reference is not a verified image digest or installed toolkit version.
- Cost values have explicit units. GCP/AWS estimates cover compute. Vast
  `cost` uses the same provider rate as normal output; `quoted_total_hourly_usd`
  separately reports the provider's total for the allocated storage when supplied,
  or `null` when absent. AWS disk size is `null` when the AMI determines it.
- `shell --print-creds` returns instance details and a `connect_command` string.
  It retains the existing readiness/key setup behavior. It can include a local
  identity-file path, but never key contents or provider credentials.

Progress, prompts and errors use stderr. Failures return a nonzero exit status
without a success document on stdout; an empty successful listing returns `[]`.
JSON listing reports fresh provider results and does not substitute cached records
when a query fails. Consumers should check the exit status and schema version and
tolerate additional fields.

`--json` selects output format; it does not accept a rental offer or bypass existing
confirmation prompts. JSON creation returns the machine details without offering
to open an interactive shell. Commands without `--json` retain normal output.

## Config

Config file: `~/.ice/config.toml`

Use:

- `ice config list` to view supported keys and current values
- `ice config get <KEY>` to read one key
- `ice config set <KEY=VALUE>` to write one key
- `ice config unset <KEY>` to clear one key

Auth values are redacted in config output.

### Config keys

- `default.cloud`: `vast.ai|gcp|aws|local`
- `default.runtime_hours`
- `default.vast_ai.min_cpus|min_ram_gb|allowed_gpus|max_price_per_hr`
- `default.gcp.min_cpus|min_ram_gb|allowed_gpus|max_price_per_hr`
- `default.aws.min_cpus|min_ram_gb|allowed_gpus|max_price_per_hr`
- `default.gcp.region|zone|image_family|image_project|boot_disk_gb`
- `default.aws.region|ami|key_name|ssh_key_path|ssh_user|security_group_id|subnet_id|root_disk_gb`
- `auth.vast_ai.api_key`
- `auth.gcp.project|service_account_json`
- `auth.aws.access_key_id|secret_access_key`

## `ice create`

`ice create` now takes exactly one deployment target mode explicitly, or defaults to local `arca`
unpack deployment when none is given.

Target modes:

- `--ssh`
- `--container IMAGE_REF`
- `--unpack SOURCE`
- `--arca [ARTIFACT]`
- bare `TARGET`

Behavior:

- `ice create test-crate` means `ice create --arca test-crate`.
- Bare `ice create` means `ice create --arca`, which selects the newest local `arca` artifact.
- `--arca NAME` is shorthand for `--unpack arca:NAME`.
- `--container` accepts a remote container image ref such as
  `us-central1-docker.pkg.dev/my-project/arca/my-image:tag`.
- `--container arca:...` is rejected because local `arca` artifacts are an unpack-only flow.
- `--unpack` accepts:
  - `arca:selector`
  - a local image name such as `arca-local:my-crate-deadbeef`
  - a saved `image.tar` path
  - a full remote container ref
- `--hours` overrides runtime duration for the deploy. If omitted, `ice` uses
  `default.runtime_hours` and otherwise falls back to `1.0`.
- `--custom` prompts for search filters on marketplace-backed clouds.
- `--machine` pins a specific marketplace machine type.
- `--dry-run` reports the chosen machine and exits before provisioning.

Examples:

```bash
ice create test-crate
ice create --arca test-crate --hours 0.25
ice create --unpack arca:test-crate --cloud vast.ai
ice create --container us-central1-docker.pkg.dev/my-project/arca/my-image:tag --cloud vast.ai
ice create --ssh --cloud gcp --machine g2-standard-4
```

## Workload behavior

- `container` runs a real container image on the target machine.
- `unpack` extracts the layer that introduced the image entrypoint plus all higher layers, uploads
  only that staged filesystem diff, and starts the image entrypoint detached with persisted logs.
- `local` supports `container` and `unpack`. It does not support `--ssh`.
- `vast.ai` container workloads use entrypoint mode, not SSH mode.
- `vast.ai`, `gcp`, and `aws` unpack workloads boot a shell-capable machine, upload the unpack
  bundle over SSH, and run the workload detached from the SSH session.
- Deploy flows print explicit stages for machine creation, SSH readiness, unpack upload, workload
  start, and log following.
- If Vast offer acceptance fails, `ice create` can immediately retry the search interactively.

## Logs and shell

- `ice logs` shows stdout and stderr for managed workloads.
- For `unpack`, logs come from the persisted detached `stdio.log` and `--follow` exits
  automatically after the workload finishes and the final output is drained.
- For `container` on `vast.ai`, logs use Vast's provider API.
- `ice shell` opens the workload shell when possible and otherwise falls back to the host shell.

## Notes

- `--cloud` can be omitted when `default.cloud` is configured.
- `ice list --cloud local` reports both managed local containers and managed local unpack workloads.
- Private GCP registry pulls use your configured GCP credentials or active `gcloud`
  authentication.
- External commands used by some flows: `ssh`, `rsync`, `gcloud`, `aws`, `docker`, `podman`.
