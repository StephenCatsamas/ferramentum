# Verda GPU VMs

Ice supports ordinary on-demand NVIDIA GPU VMs through the Verda v1 API. The
Verda CLI is not required. Supported commands are `login`, `catalog`,
`create --ssh`, `list`, `shell --print-creds`, `push`, `pull`, `start`, `stop`,
and `delete`. Interactive SSH also works. Container/Arca/unpack deployment and
managed workload logs are unsupported and fail explicitly.

## Credentials and discovery

Run this in a terminal to set up persistent credentials:

```sh
ice login --cloud verda
```

Ice opens the Verda console. Select **Credentials → Cloud API credentials →
Create**, then paste the Client ID and Client Secret at Ice's prompts. Both inputs
are hidden. Ice validates the pair with Verda before saving it under
`auth.verda.client_id` and `auth.verda.client_secret` in its config (normally
`~/.config/ice/config.toml` on Linux). The file stores credentials as plaintext
with owner-only permissions on Unix. Config display redacts both values.

Subsequent commands reuse the saved pair. `ice login --cloud verda --force`
prompts for replacements; cancellation or failed validation leaves the saved pair
unchanged. Vast and Verda share this login policy and persistence mechanism.

For automation, provide both `VERDA_CLIENT_ID` and `VERDA_CLIENT_SECRET` through
your secret manager or shell environment, or use the saved configuration.
Environment credentials take precedence even with `--force`, are validated
without prompts and are never saved. An incomplete environment pair fails
instead of mixing it with saved credentials. To replace saved credentials, unset
both environment variables before using `--force`. Noninteractive login never
opens a browser or prompts; `--force` without environment credentials requires
interactive input. Ice exchanges credentials for a short-lived OAuth token held
only in memory.

```sh
ice login --cloud verda --non-interactive --json
ice catalog --cloud verda --non-interactive --json
```

`catalog` returns live machine types, supported image types, image IDs,
availability by location, and volume prices. It includes unavailable types for
discovery; `create` intersects the catalog with current on-demand availability.
Catalog entries and provider claims do not establish profiling access.

Register your SSH public key in Verda before creating a VM. Supply its ID with
`--ssh-key-id` or `default.verda.ssh_key_id`. Before creating a billable VM, Ice
reads that key and matches its public material to an existing local or agent key.
It does not upload or modify keys. For a specific private key:

```sh
ice config set default.verda.ssh_key_id=YOUR_REGISTERED_KEY_UUID
ice config set default.verda.ssh_key_path=/absolute/path/to/private-key
```

Without a configured path, Ice searches local key pairs and then ssh-agent for a
matching key, rather than picking the first unrelated local identity. Local
checks have a ten-second budget. Keep `ssh-keygen` available; encrypted/hardware
keys need their `.pub` counterpart and a matching identity loaded with `ssh-add`,
or a matching agent-only identity when no path is configured. An explicit wrong
key path fails instead of silently selecting another key. Existing VM connection
and transfer commands use the VM's registered key IDs when available.

SSH uses root, batch authentication and `StrictHostKeyChecking=accept-new`.
The local OpenSSH availability check uses `ssh -V` (not `--version`).
Readiness uses the same bounded authentication probe as Vast. Connection refusal,
reset and similar transport failures retry within the startup deadline;
authentication, host-key, local-setup and unknown failures stop immediately with
structured diagnostics. Ice does not rewrite `known_hosts` to bypass a mismatch.

## Preview and create

```sh
ice create --cloud verda --ssh --gpu-count 1 --min-gpu-memory-gb 48 \
  --disk-gb 100 --max-price-per-hr 1 --hours 0.25 \
  --no-defaults --non-interactive --dry-run --json
```

The limits above are examples, not a current offer or spending authorization.
`--gpu` matches the catalog's model/name, case-insensitively; `--machine` pins
an exact instance type. `--min-cpus`, `--min-ram-gb`, GPU count and per-GPU memory
are enforced. Verda's catalog VRAM is the total across all GPUs; Ice divides by
GPU count for `--min-gpu-memory-gb` and reports `offer.gpu_memory_per_gpu_gb`.
Host internet-speed filters are rejected because the catalog
does not supply measured rates. CPU-only, bare-metal and confidential-computing
configurations are outside this adapter's initial creation scope.

Ice selects a currently advertised Ubuntu 24/26 image with CUDA 12.9 or newer
from the machine's `supported_os` list, preferring the highest Ubuntu/CUDA
version. Both current dotted names (`26.04.cuda13.2`) and legacy names
(`ubuntu-24.04-cuda-12.9-open`) are supported. At equal versions, a plain CUDA
image is preferred over a Docker image. Cluster, Kubernetes and
confidential-computing images are excluded. Override with
`--image IMAGE_TYPE_OR_UUID` and `--location LOCATION_CODE`, or the matching
`default.verda.image`/`location` settings. The selected image UUID is submitted
to creation. Compatibility metadata is a selection constraint, not runtime
verification of the driver, tools or counters.

`no_matching_offers` includes counts by the first failing selection stage:
resources/price, compatible image, or availability/location. It is not by itself
proof that the provider has no capacity.

The default boot disk is **100 GB**, priced as NVMe. `--disk-gb` changes both the
quoted size and the creation request. The API image catalog does not expose a
minimum disk size: this default follows the provider's CLI example, and neither
the catalog nor a preview guarantees every disk size is accepted. The separately
authorised Ada trial used 100 GB successfully; smaller overrides remain unverified.

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
Startup waits for both `running` and successful SSH authentication within one
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
or have exact-ID terminal records (`discontinued` for the VM; `deleted` with
`is_permanently_deleted: true` for its OS volume). Both paths also require absence
from the paginated active-instance/volume lists and volume trash. Additional attached volumes are retained and
listed as `retained_volume_ids`; they may continue to incur storage charges.
If the OS volume ID is unavailable, the receipt explicitly marks storage
cleanup as unknown. Inspect and remove remaining storage through Verda.

Create accepts a plain UUID, JSON UUID string, or JSON object with an `id`.
The plain-text exception is restricted to successful instance creation responses.
Create POSTs are never replayed automatically. If the response is lost or unusable, errors
include the generated hostname for reconciliation using `ice list` or the
Verda console. Once an ID is returned, recovery details preserve it and any
observed OS/attached volume IDs, plus a cleanup command. A startup failure leaves
the VM billable; it does not imply a stop or deletion. Inspect that resource
before retrying creation. A deletion timeout is also an unverified outcome;
inspect the returned VM/volume IDs rather than assuming billing ended.

An ambiguous or rejected delete receipt triggers bounded read-only reconciliation,
never a repeated mutation. Success requires all the resource checks above, and
reports `action_receipt_confirmed: false` plus sanitized `action_error` diagnostics
when reconciliation establishes deletion despite that receipt. An unverified result
retains the VM/OS/attached-volume IDs and both action and verification diagnostics.
Receipt diagnostics preserve types, field presence, row counts and ID/action
comparisons; arbitrary provider strings and unknown field names are not printed.

Safe API reads retry transient transport failures and HTTP 408/429/500/502/503/504
up to six attempts within the original request deadline. `Retry-After` is
honoured in full: if the delay cannot fit, Ice returns `verda_retry_deadline`
without sending an early retry. Backoff is interruptible; a GET can refresh an
expired token once. Resource mutations are still sent only once.

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
`--no-probe` reads connection metadata without starting the VM, probing SSH,
matching local keys, or requiring an installed SSH executable/local private key.
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
Both long `Metric Name`/`Metric Value` rows and NCU 2026.1.1's wide table with a
separate units row are supported; the admission rules are identical.
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
`admission.json`, before deletion. A separately authorised 1 October 2026 trial
captured 2,604,224 SM cycles and 458,752 executed instructions on RTX 6000 Ada
GPU 0 with Ubuntu 26.04/CUDA 13.2, driver 580.178.04, nvcc 13.2.86, NCU 2026.1.1
and root. All 1,048,576 outputs passed before, during and after capture. Its
original raw CSV is retained as a regression fixture. This verifies that context,
not A6000, non-root execution, or a future VM. The trial needed an isolated
creation hotfix and direct API cleanup verification; the corrected complete Ice
lifecycle still needs separate live validation. No new paid trial is required
to run the mocked regression suite.

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
