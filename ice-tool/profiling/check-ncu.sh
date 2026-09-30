#!/usr/bin/env bash
# Run on an explicitly authorized trial VM. Does not install tools or alter drivers.
set -euo pipefail
probe_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
probe_output=${1:?Usage: bash check-ncu.sh NEW_OUTPUT_DIRECTORY}
mkdir -- "$probe_output"
probe_output=$(cd -- "$probe_output" && pwd)
printf '%s\n' '{"admitted":false,"status":"probe_incomplete"}' > "$probe_output/admission.json"
trap 'printf "%s\n" "Admission failed; preserve $probe_output and perform the agreed VM cleanup." >&2' ERR
export PATH="/usr/local/cuda/bin:$PATH"
export CUDA_VISIBLE_DEVICES="${CUDA_VISIBLE_DEVICES:-0}"
export LC_ALL=C
probe_nvcc=${NVCC:-nvcc}
probe_ncu=${NCU:-ncu}
probe_smi=${NVIDIA_SMI:-nvidia-smi}
for probe_command in "$probe_nvcc" "$probe_ncu" "$probe_smi" timeout python3; do command -v "$probe_command" >/dev/null; done
cp -- "$probe_root/counter-probe.cu" "$probe_output/counter-probe.cu"
cd -- "$probe_output"
{
    date -u +%FT%TZ
    printf 'CUDA_VISIBLE_DEVICES=%s\n' "$CUDA_VISIBLE_DEVICES"
    timeout --kill-after=2s 10s "$probe_smi" --query-gpu=uuid,name,driver_version --format=csv
    timeout --kill-after=2s 10s "$probe_nvcc" --version
    timeout --kill-after=2s 10s "$probe_ncu" --version
} > environment.txt 2>&1
timeout --kill-after=5s 60s "$probe_nvcc" -std=c++17 -O2 -arch=native counter-probe.cu -o counter-probe > build.log 2>&1
timeout --kill-after=2s 10s ./counter-probe > before.log 2>&1
timeout --kill-after=5s 30s "$probe_ncu" --config-file off --clock-control none --replay-mode kernel \
    --kernel-name-base function --kernel-name counter_probe --launch-count 1 \
    --metrics sm__cycles_elapsed.sum,smsp__inst_executed.sum \
    --export counter-access ./counter-probe > capture.log 2>&1
test -s counter-access.ncu-rep
timeout --kill-after=2s 15s "$probe_ncu" --config-file off --import counter-access.ncu-rep --page raw --csv \
    > counters.csv 2> import.stderr
timeout --kill-after=2s 10s ./counter-probe > after.log 2>&1
python3 "$probe_root/validate_ncu.py" "$probe_output"
