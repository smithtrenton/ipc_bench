# ipc-bench

Windows 11 IPC benchmark suite inspired by [`goldsborough/ipc-bench`](https://github.com/goldsborough/ipc-bench), rebuilt around a Rust workspace and Windows-native IPC primitives.

## Scope

This suite measures **same-machine, low-level, programmable IPC** on Windows 11. It intentionally excludes GUI- and app-integration-oriented mechanisms such as Clipboard, DDE, OLE/COM automation, and `WM_COPYDATA`.

Each benchmark follows the same basic contract:

- parent/child process topology
- ping-pong round trips
- configurable message count, message size, warmups, and trials
- comparable JSON output across Rust and Python methods

## Implemented benchmark methods

| Tier | Methods |
| --- | --- |
| **Native baseline** | `copy-roundtrip` |
| **Core native** | `anon-pipe`, `named-pipe-byte-sync`, `named-pipe-message-sync`, `named-pipe-overlapped`, `tcp-loopback`, `shm-events`, `shm-semaphores`, `shm-mailbox-spin`, `shm-mailbox-hybrid`, `shm-ring-spin`, `shm-ring-hybrid` |
| **Extensions** | `af-unix`, `udp-loopback`, `mailslot`, `rpc` |
| **Experimental** | `alpc` |
| **Python baselines** | `py-multiprocessing-pipe`, `py-multiprocessing-queue`, `py-socket-tcp-loopback`, `py-shared-memory-events`, `py-shared-memory-queue` |

`copy-roundtrip` is intentionally **not IPC**. It exists as a byte-movement floor for the shared-memory request/response shape, so the main apples-to-apples IPC comparison surface remains the core native table. Extension and experimental methods are documented separately where semantics or API stability differ.

The `placeholder` benchmark remains a harness smoke target only. It is **not** part of the comparison tables.

## Building

Use the release profile for any serious measurement:

```powershell
cargo build --release --workspace
```

For correctness checks:

```powershell
cargo test --workspace
```

Python baselines target **Python 3.14** and are expected to run through **uv**. The PowerShell runners use `uv run --python 3.14 ...` for Python baselines automatically, and the Python methods implement the same CLI and JSON contract as the Rust harness.

## Running one benchmark

Native example:

```powershell
cargo run --release -p anon-pipe -- --message-count 1000 --message-size 1024 --warmup-count 100 --trials 3
```

JSON output:

```powershell
cargo run --release -p shm-ring-hybrid -- --format json
```

Copy-only baseline:

```powershell
cargo run --release -p copy-roundtrip -- --format json
```

Python example:

```powershell
uv run --python 3.14 python -m benchmarks.methods.python.py_multiprocessing_pipe.run --format json
```

## CLI contract

- `-c`, `--message-count <N>` - number of measured round trips
- `-s`, `--message-size <N>` - payload size in bytes
- `-w`, `--warmup-count <N>` - warmup iterations before timing
- `-t`, `--trials <N>` - number of benchmark trials
- `--format <text|json>` - output format

## Reproducing the full matrix

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-benchmarks.ps1
```

For the lower-noise published rerun:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-high-iteration-benchmarks.ps1
```

To reproduce the published directories exactly, remove the old output first and rerun both scripts:

```powershell
Remove-Item -Recurse -Force .\results\published\windows11-initial -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force .\results\published\windows11-high-iterations -ErrorAction SilentlyContinue
powershell -ExecutionPolicy Bypass -File .\scripts\run-benchmarks.ps1 -OutputDir results\published\windows11-initial
powershell -ExecutionPolicy Bypass -File .\scripts\run-high-iteration-benchmarks.ps1 -OutputDir results\published\windows11-high-iterations
```

Both scripts build Rust benchmarks in the **release** profile automatically.
Python benchmark execution in those scripts requires `uv`; pass `-SkipPython` if you want a Rust-only run.

Each run writes:

- `metadata.json` - machine and toolchain metadata
- `run-status.json` - overall run state, including failure details for partial or failed runs
- `manifest.json` - list of generated result files
- `summary.json` - flattened summary rows across all benchmark outputs
- `summary.csv` - CSV form of the same summary data
- one JSON report per method and message size

## Published result sets

The published repository result sets live under `results\published`.

| Result set | Purpose | Methodology | Full results |
| --- | --- | --- | --- |
| **Initial published run** | Baseline full-matrix run across all implemented methods | **Release** Rust profile; `1000` messages, `100` warmups, `3` trials; message sizes `64`, `1024`, `4096`, `16384`, `32704` | [Directory](results/published/windows11-initial), [metadata.json](results/published/windows11-initial/metadata.json), [run-status.json](results/published/windows11-initial/run-status.json), [summary.csv](results/published/windows11-initial/summary.csv), [summary.json](results/published/windows11-initial/summary.json) |
| **High-iteration run** | Lower-noise rerun for very low-latency methods | **Release** Rust profile; `100000` messages, `10000` warmups, `7` trials by default across the same message sizes; `mailslot` override of `5000` / `200` / `5` | [Directory](results/published/windows11-high-iterations), [metadata.json](results/published/windows11-high-iterations/metadata.json), [run-status.json](results/published/windows11-high-iterations/run-status.json), [summary.csv](results/published/windows11-high-iterations/summary.csv), [summary.json](results/published/windows11-high-iterations/summary.json) |

Both published sets currently contain **110 completed runs** and **0 failures**.

Both published sets were generated on:

- **OS:** Microsoft Windows 11 Pro, build 26200
- **CPU:** AMD Ryzen 9 7950X3D (16 cores / 32 logical processors)
- **Rust:** `rustc 1.94.0`, `cargo 1.94.0`
- **Python:** 3.14.3

Use `summary.json` or `summary.csv` for quick comparison, and the per-method JSON files when you need full per-trial detail.

## High-iteration results

Average round-trip latency in microseconds from `results\published\windows11-high-iterations\summary.json`. Lower is better.

| Tier | Method | 64 B | 1024 B | 4096 B | 16384 B | 32704 B |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Native baseline | `copy-roundtrip` | 0.019 | 0.042 | 0.149 | 1.348 | 2.344 |
| Core native | `anon-pipe` | 16.824 | 12.550 | 18.657 | 19.343 | 21.077 |
| Core native | `named-pipe-byte-sync` | 11.783 | 10.242 | 13.400 | 14.759 | 15.251 |
| Core native | `named-pipe-message-sync` | 12.044 | 10.091 | 13.151 | 14.190 | 16.152 |
| Core native | `named-pipe-overlapped` | 12.962 | 10.729 | 14.560 | 16.225 | 17.589 |
| Core native | `tcp-loopback` | 23.875 | 24.942 | 25.532 | 29.931 | 27.209 |
| Core native | `shm-events` | 9.833 | 9.782 | 10.621 | 11.839 | 13.449 |
| Core native | `shm-semaphores` | 9.770 | 9.816 | 10.672 | 11.841 | 13.579 |
| Core native | `shm-mailbox-spin` | 0.119 | 0.236 | 0.700 | 2.181 | 4.613 |
| Core native | `shm-mailbox-hybrid` | 0.269 | 0.307 | 0.671 | 2.196 | 4.101 |
| Core native | `shm-ring-spin` | 0.158 | 0.264 | 0.629 | 1.886 | 3.089 |
| Core native | `shm-ring-hybrid` | 0.292 | 0.347 | 0.673 | 1.887 | 3.262 |
| Extensions | `af-unix` | 13.334 | 13.518 | 14.478 | 15.836 | 16.456 |
| Extensions | `udp-loopback` | 21.499 | 21.849 | 23.093 | 25.776 | 27.496 |
| Extensions | `mailslot` | 10.650 | 11.747 | 10.827 | 12.162 | 15.524 |
| Extensions | `rpc` | 6.044 | 6.295 | 34.910 | 47.807 | 63.515 |
| Experimental | `alpc` | 5.558 | 5.610 | 6.428 | 9.858 | 14.849 |
| Python baselines | `py-multiprocessing-pipe` | 21.396 | 34.331 | 36.875 | 41.232 | 43.574 |
| Python baselines | `py-multiprocessing-queue` | 55.189 | 70.378 | 72.024 | 84.548 | 102.566 |
| Python baselines | `py-socket-tcp-loopback` | 27.140 | 26.457 | 28.199 | 33.505 | 37.442 |
| Python baselines | `py-shared-memory-events` | 56.344 | 55.059 | 53.206 | 57.935 | 63.540 |
| Python baselines | `py-shared-memory-queue` | 45.205 | 54.341 | 51.814 | 55.106 | 63.996 |

## Methodology and caveats

- **Core vs extension vs experimental matters.** Core methods are the primary comparison table. Extension methods widen Windows coverage. `alpc` is implemented, but it remains experimental because it depends on lower-level Native API surfaces.
- **Shared-memory variants are intentionally separate.** On Windows, synchronization strategy often dominates performance, so file-mapping + events, semaphores, mailbox spin, mailbox hybrid, ring spin, and ring hybrid are separate benchmark entries.
- **`copy-roundtrip` is a baseline, not a transport.** It measures the copy-only floor of the shared-memory request/response shape with no cross-process signaling or kernel transport in the loop.
- **CI is for correctness, not published performance.** GitHub Actions smoke runs verify that methods build and execute, but repository performance numbers should come from a controlled local Windows machine.
- **Python is a runtime baseline, not a direct transport match.** The Python rows are useful for overhead comparison, not as strict one-to-one equivalents for every native primitive.
- **Result interpretation should emphasize both latency and throughput.** `average_micros` is useful for round-trip latency; `message_rate` is useful for bulk throughput comparisons.

## Adding a new method

When adding a benchmark, keep the benchmark contract stable:

1. Add one executable per method under `benchmarks\methods\native\...` or `benchmarks\methods\python\...`.
2. Preserve the shared CLI, warmup behavior, trial behavior, and JSON schema.
3. Keep message semantics aligned with the existing ping-pong contract unless the method must live in the extension or experimental tier.
4. Update `scripts\run-benchmarks.ps1`, `README.md`, and CI smoke coverage when the new method becomes part of the supported matrix.
5. Document any method-specific caveats clearly, especially if the transport is one-way, framework-heavy, or lower-stability.

## Workspace layout

- `benchmarks\harness` - shared benchmark types, stats, process orchestration, and report formatting
- `benchmarks\methods\native\*` - native Rust benchmark executables and shared native support code
- `benchmarks\methods\python\*` - Python baseline scripts plus the shared adapter module
- `scripts` - benchmark automation scripts
- `results` - captured result sets, including the published Windows 11 result directories

## GitHub Actions

The Windows CI job builds the workspace, runs tests, and executes smoke runs across native Rust, the copy baseline, Python, and the experimental ALPC benchmark.
