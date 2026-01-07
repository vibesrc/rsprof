# Section 10: Security Considerations

## 10.1 Privilege Requirements

### 10.1.1 Required Capabilities

rsprof requires elevated privileges to attach to processes and use kernel tracing:

| Capability | Purpose | Fallback |
|------------|---------|----------|
| `CAP_SYS_PTRACE` | Attach to processes owned by other users | Only profile own processes |
| `CAP_PERFMON` | Use perf_event for CPU sampling | Denied if paranoid > 1 |
| `CAP_BPF` | Load eBPF programs | No heap tracking |
| `CAP_SYS_ADMIN` | Some eBPF map operations | Limited functionality |

### 10.1.2 Running as Root

Running rsprof as root grants all capabilities but introduces risks:
- Can attach to any process including security-sensitive daemons
- eBPF programs run in kernel context
- Potential for information disclosure

rsprof SHOULD warn when run as root against sensitive processes (e.g., sshd, gpg-agent).

### 10.1.3 Recommended Setup

For development use:
```bash
# Allow perf for all users
sudo sysctl kernel.perf_event_paranoid=1

# Grant capabilities to rsprof binary
sudo setcap cap_perfmon,cap_bpf,cap_sys_ptrace+ep $(which rsprof)
```

For production monitoring, use a dedicated monitoring user with minimal capabilities.

## 10.2 Process Attachment

### 10.2.1 Ownership Check

By default, Linux allows:
- Attaching to processes owned by the same user
- Attaching to child processes
- Root can attach to any process

rsprof MUST verify attachment is permitted before modifying target state.

### 10.2.2 ptrace_scope

`/proc/sys/kernel/yama/ptrace_scope` restricts attachment:

| Value | Meaning |
|-------|---------|
| 0 | Classic ptrace permissions |
| 1 | Only parent can ptrace children (Ubuntu default) |
| 2 | Only admin can ptrace |
| 3 | No ptrace allowed |

rsprof SHOULD check this value and provide helpful error messages.

### 10.2.3 Container Isolation

When targeting containerized processes:
- Container PID namespaces may hide the target
- AppArmor/SELinux may block attachment
- eBPF may be disabled in the container

rsprof SHOULD detect container environments and warn about limitations.

## 10.3 eBPF Security

### 10.3.1 Verifier

All eBPF programs pass through the kernel verifier, which ensures:
- No out-of-bounds memory access
- No infinite loops
- No unauthorized kernel data access
- Bounded execution time

rsprof's eBPF programs MUST pass the verifier without disabling checks.

### 10.3.2 Unprivileged eBPF

Kernel 5.16+ supports unprivileged eBPF with restrictions. rsprof MAY support this mode with reduced functionality:
- No kernel symbol access
- Limited map types
- Smaller map sizes

### 10.3.3 BTF Requirements

CO-RE requires BTF (BPF Type Format) in the kernel:
```bash
ls /sys/kernel/btf/vmlinux
```

If BTF is unavailable, rsprof MUST fail with a clear error rather than loading potentially incompatible BPF code.

## 10.4 Information Disclosure

### 10.4.1 Memory Contents

rsprof does not read arbitrary process memory. It only observes:
- Instruction pointers (code locations)
- Allocation sizes
- Return addresses

However, this information can reveal:
- Code paths taken (timing attacks)
- Allocation patterns
- Active features

### 10.4.2 Symbol Information

Debug symbols may contain:
- Internal function names
- File paths revealing project structure
- Variable names in DWARF info

rsprof output files SHOULD be treated as sensitive.

### 10.4.3 Multi-tenant Concerns

In shared environments:
- Users should not profile other users' processes
- Exported data should not be world-readable
- System-wide profiling requires explicit authorization

## 10.5 Denial of Service

### 10.5.1 Target Process Impact

rsprof's profiling adds overhead:
- CPU: 1-5% typical, up to 15% at high frequencies
- Memory: eBPF maps consume kernel memory
- Latency: uprobes add ~100-500ns per allocation

A malicious or misconfigured profiler could degrade target performance.

### 10.5.2 Kernel Resource Exhaustion

eBPF maps consume kernel memory. rsprof MUST:
- Set reasonable map size limits
- Clean up maps on exit
- Handle `ENOMEM` gracefully

### 10.5.3 Ring Buffer Overflow

If rsprof cannot keep up with events, the ring buffer overflows. This is a data loss issue, not a security issue, but:
- Lost samples may hide performance problems
- High overflow rates indicate misconfiguration

## 10.6 Supply Chain

### 10.6.1 Dependencies

rsprof depends on several crates. Critical dependencies:
- `libbpf-rs` - eBPF loading (unsafe code)
- `gimli` - DWARF parsing (complex parsing)
- `object` - ELF parsing (complex parsing)

These SHOULD be audited and pinned to specific versions.

### 10.6.2 eBPF Program Source

The embedded eBPF program is compiled at build time. rsprof MUST:
- Include BPF source in the repository
- Document build process
- Provide reproducible builds

### 10.6.3 Binary Distribution

Pre-built binaries SHOULD be:
- Signed with a verifiable key
- Built from tagged releases
- Reproducibly built where possible
