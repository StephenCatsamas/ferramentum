# Verda GPU VMs

Ice supports ordinary on-demand NVIDIA GPU VMs through the Verda v1 API. The
Verda CLI is not required. Supported commands are `login`, `catalog`,
`create --ssh`, `list`, `shell --print-creds`, `push`, `pull`, `start`, `stop`,
and `delete`. Interactive SSH also works. Container/Arca/unpack deployment and
managed workload logs are unsupported and fail explicitly.

## Credentials and discovery

Create API client credentials in the Verda console and provide both
`VERDA_CLIENT_ID` and `VERDA_CLIENT_SECRET` through your secret manager or shell
environment. Ice exchanges them for a short-lived OAuth token in memory; it does
not save environment credentials. Alternatively configure
`auth.verda.client_id` and `auth.verda.client_secret` in Ice's config. An incomplete
environment pair fails instead of mixing it with saved credentials. Config
display redacts both fields. `login --force` also validates a fresh token.

```sh
ice login --cloud verda --non-interactive --json
ice catalog --cloud verda --non-interactive --json
```

`catalog` returns live machine types, supported image types, image IDs,
availability by location, and volume prices. It includes unavailable types for
discovery; `create` intersects the catalog with current on-demand availability.
Catalog entries and provider claims do not establish profiling access.

Register your SSH public key in Verda before creating a VM. Supply its ID with
`--ssh-key-id` or `default.verda.ssh_key_id`. Ice verifies that the ID exists in
the project, but does not upload or modify keys. For a specific private key:

```sh
ice config set default.verda.ssh_key_id=YOUR_REGISTERED_KEY_UUID
ice config set default.verda.ssh_key_path=/absolute/path/to/private-key
```

Without a configured path, Ice uses an existing local SSH key pair when found,
then the SSH agent/default identity mechanism. The provider key ID must correspond
to an accessible local key. SSH uses root on these images, batch authentication
and `StrictHostKeyChecking=accept-new`. Existing host-key mismatches fail.

## Preview and create

```sh
ice create --cloud verda --ssh --gpu-count 1 --min-gpu-memory-gb 48 \
  --disk-gb 100 --max-price-per-hr 1 --hours 0.25 \
  --no-defaults --non-interactive --dry-run --json
```

The limits above are examples, not a current offer or spending authorization.
`--gpu` matches the catalog's model/name, case-insensitively; `--machine` pins
an exact instance type. `--min-cpus`, `--min-ram-gb`, GPU count and per-GPU memory
are enforced. Host internet-speed filters are rejected because the catalog
does not supply measured rates. CPU-only, bare-metal and confidential-computing
configurations are outside this adapter's initial creation scope.

Ice selects a currently advertised Ubuntu 24/26 image with CUDA 12.9 or newer
from the machine's `supported_os` list, preferring the highest Ubuntu/CUDA
version. Cluster and confidential-computing images are excluded. Override with
`--image IMAGE_TYPE_OR_UUID` and `--location LOCATION_CODE`, or the matching
`default.verda.image`/`location` settings. The selected image UUID is submitted
to creation. Compatibility metadata is a selection constraint, not runtime
verification of the driver, tools or counters.

The default boot disk is **100 GB**, priced as NVMe. `--disk-gb` changes both the
quoted size and the creation request. The API image catalog does not expose a
minimum disk size: this default follows the provider's CLI example, and neither
it nor a smaller override has been live-validated by this adapter.

`--max-price-per-hr` covers **compute plus the allocated OS disk** in USD. Storage
uses Verda CLI's conversion: monthly USD/GB × disk GB ÷ 730, rounded up to four
decimal places after multiplication. Unknown/invalid prices or non-USD storage
quotes fail. Bandwidth and taxes are excluded. Quotes are estimates, not an
invoice or prepaid-balance requirement; Verda documents advance billing in
10-minute increments with unused portions refunded after deletion.

Saved search settings live under `default.verda.*` in the standard Ice config.
JSON reports effective filters and their sources. `--no-defaults` ignores saved
resource filters, disk size, image and location; credentials and SSH key settings
remain usable. Creation does not save the invocation's selections.

For an authorized rental, replace `--dry-run` with
`--yes --manual-cleanup --startup-timeout 5m`. Ice refreshes availability,
image compatibility and the storage-inclusive quote immediately before creation.
A price increase since confirmation fails instead of silently accepting it.
The API offers no atomic price reservation; a preview does not reserve capacity.
Startup waits for both `running` and a successful SSH `true` probe within one
readiness budget. Creation itself is a separate, bounded API request.

## Deadlines, failures and deletion

**There is no automatic stop or deletion in this adapter.** `--hours` supplies a
cost estimate and `cleanup.planned_deadline_unix`, not an enforced deadline.
Actual creation requires `--manual-cleanup`, including with `--yes`. Coordinate
an independent deadline/deletion guard before a paid trial and verify cleanup.
The JSON receipt reports `deadline_enforced: false`, `automatic_stop: false`,
and `automatic_delete: false`.

Verda `stop` means `shutdown`: **compute and storage billing continue** while
the VM is offline. `start` maps to `start`; it does not create a new deadline.
Use `delete` to end compute billing. Ice requests permanent deletion of the
selected VM's OS volume and verifies that the VM and OS volume are absent,
including checking volume trash. Additional attached volumes are retained and
listed as `retained_volume_ids`; they may continue to incur storage charges.
If the OS volume ID is unavailable, the receipt explicitly marks storage
cleanup as unknown. Inspect and remove remaining storage through Verda.

Create POSTs are never replayed automatically. If the response is lost, errors
include the generated hostname for reconciliation using `ice list` or the
Verda console. Once an ID is returned, recovery details preserve it and any
observed OS/attached volume IDs, plus a cleanup command. A startup failure leaves
the VM billable; it does not imply a stop or deletion. Inspect that resource
before retrying creation. A deletion timeout is also an unverified outcome;
inspect the returned VM/volume IDs rather than assuming billing ended.

```sh
ice list --cloud verda --json
ice shell --cloud verda INSTANCE_UUID --print-creds --json
ice shell --cloud verda INSTANCE_UUID --print-creds --no-probe --json
ice push --cloud verda INSTANCE_UUID ./input /root/input --json
ice pull --cloud verda INSTANCE_UUID /root/results ./results --json
ice stop --cloud verda INSTANCE_UUID --json
ice start --cloud verda INSTANCE_UUID --json
ice delete --cloud verda INSTANCE_UUID --json
```

List discovery shows Ice-prefixed hostnames. Explicit full resource IDs can
address other VMs in the project; only operate resources authorized for the task.
`--no-probe` reads connection metadata without starting the VM or probing SSH.
Transfers use rsync with protected arguments and require a running VM, SSH access
and rsync at both ends. Noninteractive connection lookup never starts a stopped VM.

## Profiling admission: a capture is required

All VM receipts report profiling as **unverified**, even after SSH succeeds.
Verda documents enabled counters on current images; installation, compatibility
and that claim are not admission evidence. The included probe is a separate,
explicit operation on an authorized trial VM, not an automatic creation side effect.

Copy the [profiling directory](../profiling) to the VM using `ice push`, then run
the following over the SSH connection returned by Ice, with a new output path:

```sh
timeout --kill-after=5s 180s bash check-ncu.sh /root/ncu-admission-01
```

The checked CUDA fixture uses 8 MiB device storage, launches one integer kernel
and checks every result (including unsigned wraparound). It compiles native code
for the selected GPU, checks ordinary execution before and after capture, and
checks results during NCU capture. The script imports the resulting `.ncu-rep`
and requires positive numerical `sm__cycles_elapsed.sum` and
`smsp__inst_executed.sum` counters for that kernel in the same capture/device.
An exit-zero permission failure, missing/empty report, missing counter, NaN,
zero counter or failed output check cannot pass. GPU UUID consistency and
source/binary/report/log hashes are recorded in `admission.json`.

`CUDA_VISIBLE_DEVICES` selects the GPU (default `0`); `NVCC`, `NCU` and
`NVIDIA_SMI` can select explicit tool paths. Tools must already be installed.
The probe does not install software, elevate privileges, change driver policy,
lock clocks, or retry failed captures. A failed/unsupported capture is a blocker;
retain evidence and apply the agreed cleanup. Admission applies only to the
captured GPU/image/driver/user/tool context, not every Verda instance.

Pull the entire output directory locally, including `.ncu-rep`, CSV, logs and
`admission.json`, before deletion. Mock tests validate the gate, but **no paid VM,
SSH transfer, real CUDA compilation or NCU capture has been validated for this
adapter yet**. The installed Ice binary is not changed by this patch.

## Validation and references

```sh
cargo test -p ice-tool
python3 -m unittest discover -s ice-tool/profiling -p 'test_*.py' -v
bash -n ice-tool/profiling/check-ncu.sh
```

Rust tests use an HTTP mock server; profiling tests use fake compiler/profiler
executables and the Python standard library. No rental or system package install
is required. This workspace may need the Ice-only dependency workaround described
in the repository's validation documentation when sibling crates are unavailable:
`bash scripts/check-cli.sh ice-tool test --offline`.

- [Verda API and OpenAPI schema](https://api.verda.com/v1/docs)
- [Verda CLI instances and images](https://docs.verda.com/cli/instances/)
- [Verda CLI storage-price calculation](https://github.com/verda-cloud/verda-cli/blob/main/internal/verda-cli/cmd/util/pricing.go)
- [Billing](https://docs.verda.com/welcome-to-verda/pricing-and-billing/)
- [Provider profiling claim and prerequisites](https://docs.verda.com/cpu-and-gpu-instances/profile-with-nsight/)
