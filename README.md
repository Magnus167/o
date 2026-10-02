# o

## Features

- [x] System summary
  - CPU usage (global)
  - Memory and swap usage
  - Uptime and load average
- [x] JSON output
  - `--json` or `-j` prints machine-readable summary
- [x] Rich, colored output
  - Threshold highlighting
  - Inline usage bars
  - Aligned columns
- [ ] Top processes
  - Show top N by CPU or memory
- [x] Disk speed
  - Read/write B/s
  - Per-device detail (`-I`, `--io-detail`)
  - Adjustable sampling window (`--sample-ms N`)
- [ ] Disk usage
  - Total/used per mount
- [x] Network I/O
  - Tx/Rx
- [x] Watch/refresh mode
  - Live updates at an interval
- [x] Human-readable units
  - KB/MB/GB
- [ ] Configurable output
  - Flags and env

## Usage

Run:

```bash
o -h
```

for the built-in usage and examples.

## Install

- From source:

```bash
cargo install --path .
```

- From git:

```bash
cargo install --git https://github.com/Magnus167/o
```
