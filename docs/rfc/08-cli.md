# Section 8: Command Line Interface

## 8.1 Overview

rsprof uses a subcommand-style CLI with sensible defaults. The default behavior (no subcommand) is to record.

## 8.2 Recording

### 8.2.1 Basic Usage

```bash
# By PID
rsprof --pid 123456

# By process name (uses pgrep-style matching)
rsprof --process my-app
rsprof -p my-app
```

### 8.2.2 Output File

Default output: `rsprof.{process_name}.{YYMMDDhhmmss}.db`

```bash
# Custom output path
rsprof --pid 123456 -o profile.db
rsprof --pid 123456 --output /tmp/debug.db
```

### 8.2.3 Recording Options

```bash
rsprof --pid 123456 \
    --interval 1s \        # Checkpoint interval (default: 1s)
    --duration 5m \        # Stop after duration (default: until Ctrl-C)
    --cpu-freq 99 \        # CPU sampling frequency in Hz (default: 99)
    --quiet                # No TUI, just record
```

### 8.2.4 Process Matching

`--process` uses substring matching against `/proc/*/comm`:

```bash
rsprof --process my-app     # Matches "my-app", "my-app-worker", etc.
```

If multiple processes match, rsprof lists them and exits:

```
Multiple processes match 'app':
  PID 1234: my-app
  PID 5678: app-server
  PID 9012: app-worker
Use --pid to specify exactly one.
```

## 8.3 Viewing

### 8.3.1 Top Command

```bash
rsprof top cpu profile.db      # Top CPU consumers
rsprof top heap profile.db     # Top heap consumers
```

### 8.3.2 Filtering Options

```bash
rsprof top cpu profile.db \
    --top 50 \                 # Number of entries (default: 20)
    --threshold 0.5 \          # Only show >0.5% of total (default: 0)
    --since 30s \              # Only last 30s of recording
    --until 1m \               # Only first 1m of recording
    --window 30s..1m           # Time range (alternative syntax)
```

### 8.3.3 Output Formats

```bash
rsprof top cpu profile.db              # Human-readable table
rsprof top cpu profile.db --json       # JSON output
rsprof top cpu profile.db --csv        # CSV output
```

## 8.4 Query (Optional)

Direct SQL access for advanced analysis:

```bash
rsprof query profile.db "SELECT * FROM meta"
rsprof query profile.db "
    SELECT file, line, SUM(count) 
    FROM cpu_samples c 
    JOIN symbols s ON c.addr = s.addr 
    GROUP BY addr 
    ORDER BY 3 DESC 
    LIMIT 10
"
```

## 8.5 Full CLI Specification

```
rsprof - Zero-instrumentation profiler for Rust

USAGE:
    rsprof [OPTIONS] --pid <PID>
    rsprof [OPTIONS] --process <NAME>
    rsprof top <cpu|heap> <FILE> [OPTIONS]
    rsprof query <FILE> <SQL>

RECORDING OPTIONS:
    -p, --pid <PID>           Process ID to profile
    -P, --process <NAME>      Process name to profile (pgrep-style)
    -o, --output <FILE>       Output database path
    -i, --interval <DURATION> Checkpoint interval [default: 1s]
    -d, --duration <DURATION> Recording duration [default: unlimited]
        --cpu-freq <HZ>       CPU sampling frequency [default: 99]
    -q, --quiet               Disable TUI, record only

TOP OPTIONS:
    -n, --top <N>             Number of entries [default: 20]
    -t, --threshold <PCT>     Minimum percentage to display [default: 0]
        --since <DURATION>    Only include last N seconds
        --until <DURATION>    Only include first N seconds
        --window <RANGE>      Time range (e.g., "30s..1m")
        --json                Output as JSON
        --csv                 Output as CSV

GENERAL OPTIONS:
    -h, --help                Print help
    -V, --version             Print version

EXAMPLES:
    # Record process by PID, live TUI
    rsprof --pid 123456

    # Record process by name, custom output
    rsprof --process my-app -o debug.db

    # Record for 5 minutes, no TUI
    rsprof --pid 123456 --duration 5m --quiet

    # View top CPU consumers
    rsprof top cpu profile.db

    # View top 50 heap consumers from last 30s
    rsprof top heap profile.db --top 50 --since 30s

    # Export as JSON
    rsprof top cpu profile.db --json > report.json
```

## 8.6 Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Invalid arguments |
| 3 | Process not found |
| 4 | Permission denied |
| 5 | Missing debug info |
| 6 | Database error |

## 8.7 Signals

| Signal | Behavior |
|--------|----------|
| `SIGINT` (Ctrl-C) | Graceful shutdown, finalize database |
| `SIGTERM` | Same as SIGINT |
| `SIGQUIT` (Ctrl-\) | Immediate exit, database may be incomplete |

## 8.8 Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RSPROF_OUTPUT_DIR` | Default directory for output files | `.` |
| `RSPROF_INTERVAL` | Default checkpoint interval | `1s` |
| `RSPROF_CPU_FREQ` | Default CPU sampling frequency | `99` |
| `NO_COLOR` | Disable colored output | unset |

## 8.9 Duration Syntax

Durations accept:
- `30s` - 30 seconds
- `5m` - 5 minutes
- `2h` - 2 hours
- `1h30m` - 1 hour 30 minutes
- `90` - 90 seconds (bare number)

## 8.10 Autocompletion

Generate shell completions:

```bash
rsprof completions bash > /etc/bash_completion.d/rsprof
rsprof completions zsh > ~/.zfunc/_rsprof
rsprof completions fish > ~/.config/fish/completions/rsprof.fish
```
