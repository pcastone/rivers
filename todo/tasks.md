# Tasks — Add stop and graceful commands to riversctl

**Source:** User request — riversctl needs `stop` (immediate) and `graceful` (graceful stop) commands
**Scope:** Add two new admin API commands to riversctl

---

## Implementation

### Step 1: Add `stop` command
- [x] Add `"stop"` to the match in `main()` — calls admin API `POST /admin/shutdown` with `{"mode": "immediate"}`
- [x] Add `cmd_stop()` function
- [x] Add to `print_usage()` help text

### Step 2: Add `graceful` command
- [x] Add `"graceful"` to the match in `main()` — calls admin API `POST /admin/shutdown` with `{"mode": "graceful"}`
- [x] Add `cmd_graceful()` function
- [x] Add to `print_usage()` help text

### Step 3: Add signal-based fallback
- [x] If admin API is unreachable, fall back to sending signals:
  - `stop` → find riversd PID and send SIGKILL
  - `graceful` → find riversd PID and send SIGTERM
- [x] PID discovery: check for riversd process via `sysinfo` or read from a PID file

---
