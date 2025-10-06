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
- [ ] Network I/O
  - Tx/Rx
- [ ] Watch/refresh mode
  - Live updates at an interval
- [x] Human-readable units
  - KB/MB/GB
- [ ] Configurable output
  - Flags and env

## Install

- From source:
  ```
  cargo install --path .
  ```
