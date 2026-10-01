# NCU CSV fixtures

`ncu-long.csv` is a synthetic legacy raw-metric table used by the original gate.
`ncu-2026.1.1-wide.csv` is the unmodified raw CSV from the separately authorised
1 October 2026 Verda Ada trial, imported from a 33,069-byte NCU report. The table
includes a units row and unrelated metadata/counters. Its checked `counter_probe`
capture has 2,604,224 SM cycles and 458,752 executed instructions on GPU 0.

The source evidence lives in the companion research workspace at
`results/profiling/2026-10-01/verda-ada-trial-01/capture/`. Context: RTX 6000 Ada,
Ubuntu 26.04/CUDA 13.2, driver 580.178.04, NCU 2026.1.1, root user. This fixture
tests parsing only; it cannot establish access on a future VM.
