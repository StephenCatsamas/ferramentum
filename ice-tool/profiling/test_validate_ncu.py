from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

from validate_ncu import numeric_counters

FIXTURES = Path(__file__).resolve().parent / "fixtures"
CSV = (FIXTURES / "ncu-long.csv").read_text()
WIDE = ('"ID","Kernel Name","Device","sm__cycles_elapsed.sum","smsp__inst_executed.sum"\n'
        '"","","","cycle","inst"\n'
        '"0","counter_probe(unsigned int*)","0","12,345","64000"\n')


class AdmissionTests(unittest.TestCase):
    def test_requires_both_positive_hardware_counters(self) -> None:
        self.assertEqual(numeric_counters(CSV)["sm__cycles_elapsed.sum"], 12345)
        for invalid in (CSV.replace('"12,345"', '"0"'), CSV.replace('"12,345"', '"NaN"'),
                        CSV.replace('"12,345"', '"N/A"'), CSV.replace("smsp__inst_executed.sum", "gpu__time_duration.sum"),
                        CSV.replace("counter_probe", "different_kernel"), "==ERROR== ERR_NVGPUCTRPERM",
                        CSV + "==ERROR== import failed\n"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                numeric_counters(invalid)

    def test_counters_must_belong_to_one_capture(self) -> None:
        with self.assertRaises(ValueError):
            numeric_counters(CSV.replace('"0","counter_probe', '"1","counter_probe', 1))
        with self.assertRaises(ValueError):
            numeric_counters(CSV + CSV.splitlines()[-1] + "\n")

    def test_both_layouts_enforce_identical_admission_rules(self) -> None:
        self.assertEqual(numeric_counters(CSV), numeric_counters(WIDE))
        for text in (CSV, WIDE):
            for invalid in (text.replace('"12,345"', '"0"'), text.replace('"12,345"', '"-1"'),
                            text.replace('"12,345"', '"NaN"'), text.replace('"12,345"', '"inf"'),
                            text.replace('"12,345"', '"N/A"'), text.replace('"12,345"', '""'),
                            text.replace("smsp__inst_executed.sum", "gpu__time_duration.sum"),
                            text.replace("counter_probe", "other_kernel"),
                            text + text.splitlines()[-1] + "\n",
                            text + text.splitlines()[-1].replace('"0","counter_probe', '"1","counter_probe') + "\n",
                            text.replace('","0",', '","",'),
                            text.replace('"Kernel Name",', '"Device",'),
                            text + "==ERROR== import failed\n"):
                with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                    numeric_counters(invalid)

    def test_recorded_ncu_2026_wide_csv(self) -> None:
        self.assertEqual(numeric_counters((FIXTURES / "ncu-2026.1.1-wide.csv").read_text()),
                         {"sm__cycles_elapsed.sum": 2604224, "smsp__inst_executed.sum": 458752})

    def run_mock_probe(self, root: Path, denied: bool, csv_text: str = CSV) -> subprocess.CompletedProcess[str]:
        tools = root / "tools"
        tools.mkdir()
        source = Path(__file__).resolve().parent
        probe = "#!/bin/sh\necho 'GPU UUID: 11111111111111111111111111111111'\necho 'PASS: all 1048576 unsigned outputs checked'\n"
        nvcc = ("#!/bin/sh\n[ \"$1\" != --version ] || exit 0\n"
                f"printf %s {shlex.quote(probe)} > counter-probe\nchmod +x counter-probe\n")
        ncu = ("#!/bin/sh\n[ \"$1\" != --version ] || exit 0\n"
               f"case \"$*\" in *--import*) printf '%s' {shlex.quote(csv_text)}; exit 0 ;; esac\n")
        if denied:
            # Reproduce an exit-zero profiler permission failure. It must not pass.
            ncu += "echo '==ERROR== ERR_NVGPUCTRPERM'\nexit 0\n"
        else:
            ncu += "printf report-fixture > counter-access.ncu-rep\n./counter-probe\n"
        for name, script in {"nvcc": nvcc, "ncu": ncu, "nvidia-smi": "#!/bin/sh\necho 'fixture GPU'\n"}.items():
            path = tools / name
            path.write_text(script)
            path.chmod(0o700)
        env = dict(os.environ, NVCC=str(tools / "nvcc"), NCU=str(tools / "ncu"), NVIDIA_SMI=str(tools / "nvidia-smi"))
        return subprocess.run(["bash", str(source / "check-ncu.sh"), str(root / "output")],
                              env=env, capture_output=True, text=True, timeout=10, check=False)

    def test_probe_imports_report_and_records_checksums(self) -> None:
        for csv_text in (CSV, WIDE):
            with self.subTest(csv_text=csv_text), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                result = self.run_mock_probe(root, False, csv_text)
                self.assertEqual(result.returncode, 0, result.stderr)
                admission = json.loads((root / "output/admission.json").read_text())
                self.assertTrue(admission["admitted"])
                self.assertEqual(len(admission["sha256"]["counter-access.ncu-rep"]), 64)

    def test_exit_zero_permission_error_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = self.run_mock_probe(root, True)
            self.assertNotEqual(result.returncode, 0)
            admission = json.loads((root / "output/admission.json").read_text())
            self.assertFalse(admission["admitted"])


if __name__ == "__main__":
    unittest.main()
