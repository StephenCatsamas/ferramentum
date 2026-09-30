"""Validate evidence produced by check-ncu.sh; standard library only."""
from __future__ import annotations

import csv
import hashlib
import io
import json
import math
from pathlib import Path
import sys
from typing import Any

METRICS = ("sm__cycles_elapsed.sum", "smsp__inst_executed.sum")
PASS = "PASS: all 1048576 unsigned outputs checked"


def numeric_counters(text: str) -> dict[str, float]:
    if "ERR_" in text or "==ERROR==" in text:
        raise ValueError("NCU reported an error while importing counters")
    rows = list(csv.reader(io.StringIO(text)))
    required = {"ID", "Kernel Name", "Metric Name", "Metric Value", "Device"}
    header_index = next((i for i, row in enumerate(rows) if required.issubset(row)), None)
    if header_index is None:
        raise ValueError("No NCU raw metric table")
    header = rows[header_index]
    captures: dict[tuple[str, str], dict[str, float]] = {}
    for row in rows[header_index + 1:]:
        if len(row) != len(header):
            continue
        entry = dict(zip(header, row))
        if "counter_probe(" not in entry["Kernel Name"] and entry["Kernel Name"] != "counter_probe":
            continue
        metric = entry["Metric Name"]
        if metric not in METRICS:
            continue
        number = float(entry["Metric Value"].replace(",", ""))
        if not math.isfinite(number) or number <= 0:
            raise ValueError(f"Nonpositive or invalid counter: {metric}")
        capture = captures.setdefault((entry["ID"], entry["Device"]), {})
        if metric in capture:
            raise ValueError("Duplicate counter; expected one kernel capture")
        capture[metric] = number
    if len(captures) != 1:
        raise ValueError("Expected one checked kernel capture on one device")
    counters = next(iter(captures.values()))
    if set(counters) != set(METRICS):
        raise ValueError("The NCU report must contain both numerical hardware counters")
    return counters


def validate(directory: Path) -> dict[str, Any]:
    report = directory / "counter-access.ncu-rep"
    if not report.is_file() or report.stat().st_size == 0:
        raise ValueError("NCU report missing or empty")
    for name in ("before.log", "capture.log", "after.log"):
        text = (directory / name).read_text()
        if PASS not in text or "ERR_" in text or "==ERROR==" in text or "No kernels were profiled" in text:
            raise ValueError(f"Checked CUDA execution/capture failed: {name}")
    import_errors = (directory / "import.stderr").read_text()
    if "ERR_" in import_errors or "==ERROR==" in import_errors:
        raise ValueError("NCU report import failed")
    # Correlate the device UUID emitted by the checked application at every stage.
    uuids = [next((line for line in (directory / name).read_text().splitlines()
                   if line.startswith("GPU UUID: ")), "")
             for name in ("before.log", "capture.log", "after.log")]
    if not uuids[0] or len(set(uuids)) != 1:
        raise ValueError("GPU identity missing or changed between checks")
    counters = numeric_counters((directory / "counters.csv").read_text())
    artifacts = ("counter-probe.cu", "counter-probe", "counter-access.ncu-rep", "counters.csv",
                 "environment.txt", "before.log", "capture.log", "after.log", "import.stderr")
    hashes = {name: hashlib.sha256((directory / name).read_bytes()).hexdigest() for name in artifacts}
    return {"admitted": True, "status": "hardware_counters_verified", "gpu_uuid": uuids[0][10:],
            "kernel": "counter_probe", "counters": counters, "sha256": hashes,
            "scope": "This captured GPU, image, driver, user and tool context only; no lifecycle guarantee."}


def main() -> int:
    directory = Path(sys.argv[1])
    try:
        result = validate(directory)
    except (ValueError, OSError) as error:
        result = {"admitted": False, "status": "failed", "reason": str(error)}
    (directory / "admission.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    return 0 if result["admitted"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
