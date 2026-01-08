// SPDX-License-Identifier: GPL-2.0 OR MIT
// Heap profiling eBPF program for rsprof
// Attaches uprobes to __rust_alloc, __rust_dealloc, __rust_realloc

// Define target arch before includes
#define __TARGET_ARCH_x86

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>

// Additional PT_REGS macros not in vmlinux.h
#ifndef PT_REGS_FP
#define PT_REGS_FP(x) ((x)->rbp)  // Frame pointer
#endif
#ifndef PT_REGS_SP
#define PT_REGS_SP(x) ((x)->rsp)  // Stack pointer
#endif

// Maximum number of tracked allocations
#define MAX_ALLOCS 1000000
// Maximum number of tracked callsites
#define MAX_CALLSITES 10000
// Inline stack depth for frame pointer walking (need enough to reach user code)
// Deep call stacks like sorting need more frames to reach user code
#define INLINE_STACK_DEPTH 32

// Allocation info stored per pointer - includes inline stack for userspace filtering
struct alloc_info {
    u64 size;                         // Allocation size
    u64 stack[INLINE_STACK_DEPTH];    // Inline stack (frame pointer walk)
    u8 stack_len;                     // Number of valid entries in stack
};

// Stats per callsite
struct heap_stats {
    s64 live_bytes;     // Current live bytes (can be negative temporarily)
    u64 total_allocs;   // Total allocation count
    u64 total_frees;    // Total free count
    u64 total_alloc_bytes;  // Total bytes ever allocated
    u64 total_free_bytes;   // Total bytes ever freed
};

// Event sent to userspace via ring buffer
struct heap_event {
    u64 user_addr;      // User-level return address
    u64 ptr;            // Allocation pointer
    s64 size;           // Positive for alloc, negative for free
    u8 event_type;      // 0=alloc, 1=free, 2=realloc
};

// Map: ptr -> alloc_info (tracks live allocations)
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_ALLOCS);
    __type(key, u64);
    __type(value, struct alloc_info);
} live_allocs SEC(".maps");

// Map: user_addr -> heap_stats (aggregated stats per callsite)
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_CALLSITES);
    __type(key, u64);
    __type(value, struct heap_stats);
} heap_stats SEC(".maps");

// Ring buffer for events to userspace
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 256 * 1024); // 256KB
} events SEC(".maps");

// Per-CPU storage for passing data between entry and return probes
struct alloc_scratch {
    u64 size;
    u64 stack[INLINE_STACK_DEPTH];    // Inline stack from FP walking
    u8 stack_len;                     // Number of valid entries
};

struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, u32);
    __type(value, struct alloc_scratch);
} alloc_size_scratch SEC(".maps");

// Per-CPU storage for realloc info
struct realloc_info {
    u64 old_ptr;
    u64 old_size;
    u64 new_size;
    u64 stack[INLINE_STACK_DEPTH];
    u8 stack_len;
};

struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, u32);
    __type(value, struct realloc_info);
} realloc_scratch SEC(".maps");

// Target PID to filter (set via map from userspace)
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, u32);
    __type(value, u32);
} target_pid_map SEC(".maps");

static __always_inline u32 get_target_pid(void) {
    u32 key = 0;
    u32 *pid = bpf_map_lookup_elem(&target_pid_map, &key);
    return pid ? *pid : 0;
}

// Debug counter - increments on every probe hit (even wrong PID)
// Keys: 0=entry_all, 1=entry_pid_match, 2=ret_all, 3=ret_pid_match
//       4=first_seen_pid, 5=first_target_pid
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 6);
    __type(key, u32);
    __type(value, u64);
} debug_counters SEC(".maps");

// Entry: capture size and inline stack for __rust_alloc(size, align)
SEC("uprobe/rust_alloc_entry")
int uprobe_rust_alloc(struct pt_regs *ctx) {
    u32 pid = bpf_get_current_pid_tgid() >> 32;
    u32 target_pid = get_target_pid();
    if (target_pid && pid != target_pid)
        return 0;

    u64 size = PT_REGS_PARM1(ctx);

    struct alloc_scratch scratch = {
        .size = size,
        .stack_len = 0,
    };

    // Walk frame pointers to capture inline stack
    u64 fp = PT_REGS_FP(ctx);

    #pragma unroll
    for (int i = 0; i < INLINE_STACK_DEPTH && fp != 0; i++) {
        u64 ret_addr = 0;
        if (bpf_probe_read_user(&ret_addr, sizeof(ret_addr), (void *)(fp + 8)) != 0)
            break;

        if (ret_addr != 0) {
            scratch.stack[scratch.stack_len] = ret_addr;
            scratch.stack_len++;
        }

        u64 next_fp = 0;
        if (bpf_probe_read_user(&next_fp, sizeof(next_fp), (void *)fp) != 0)
            break;
        if (next_fp <= fp)
            break;
        fp = next_fp;
    }

    // Fallback: if no frames captured, try immediate caller from rsp
    if (scratch.stack_len == 0) {
        u64 ret_addr = 0;
        bpf_probe_read_user(&ret_addr, sizeof(ret_addr), (void *)PT_REGS_SP(ctx));
        if (ret_addr != 0) {
            scratch.stack[0] = ret_addr;
            scratch.stack_len = 1;
        }
    }

    u32 zero = 0;
    bpf_map_update_elem(&alloc_size_scratch, &zero, &scratch, BPF_ANY);

    return 0;
}

// Return: record allocation with size and stack from scratch map
SEC("uretprobe/rust_alloc_ret")
int uretprobe_rust_alloc(struct pt_regs *ctx) {
    u32 pid = bpf_get_current_pid_tgid() >> 32;
    u32 target_pid = get_target_pid();
    if (target_pid && pid != target_pid)
        return 0;

    u64 ptr = PT_REGS_RC(ctx);
    if (!ptr)
        return 0;

    // Get size and stack from scratch map (captured at entry)
    u32 zero = 0;
    struct alloc_scratch *scratch = bpf_map_lookup_elem(&alloc_size_scratch, &zero);
    if (!scratch)
        return 0;
    u64 size = scratch->size;

    // Skip if we couldn't capture any stack
    if (scratch->stack_len == 0)
        return 0;

    // Use first stack frame as key (we'll filter better in userspace)
    u64 key_addr = scratch->stack[0];

    // Store allocation info with full stack for later filtering
    struct alloc_info info = {
        .size = size,
        .stack_len = scratch->stack_len,
    };
    #pragma unroll
    for (int i = 0; i < INLINE_STACK_DEPTH; i++) {
        info.stack[i] = (i < scratch->stack_len) ? scratch->stack[i] : 0;
    }
    bpf_map_update_elem(&live_allocs, &ptr, &info, BPF_ANY);

    // Update stats for this callsite
    struct heap_stats *stats = bpf_map_lookup_elem(&heap_stats, &key_addr);
    if (stats) {
        __sync_fetch_and_add(&stats->live_bytes, size);
        __sync_fetch_and_add(&stats->total_allocs, 1);
        __sync_fetch_and_add(&stats->total_alloc_bytes, size);
    } else {
        struct heap_stats new_stats = {
            .live_bytes = size,
            .total_allocs = 1,
            .total_frees = 0,
            .total_alloc_bytes = size,
            .total_free_bytes = 0,
        };
        bpf_map_update_elem(&heap_stats, &key_addr, &new_stats, BPF_NOEXIST);
    }

    // Send event to userspace
    struct heap_event *event = bpf_ringbuf_reserve(&events, sizeof(*event), 0);
    if (event) {
        event->user_addr = key_addr;
        event->ptr = ptr;
        event->size = size;
        event->event_type = 0;  // alloc
        bpf_ringbuf_submit(event, 0);
    }

    return 0;
}

// __rust_dealloc(ptr: *mut u8, size: usize, align: usize)
SEC("uprobe/rust_dealloc")
int uprobe_rust_dealloc(struct pt_regs *ctx) {
    u32 pid = bpf_get_current_pid_tgid() >> 32;
    u32 target_pid = get_target_pid();
    if (target_pid && pid != target_pid)
        return 0;

    u64 ptr = PT_REGS_PARM1(ctx);
    u64 size = PT_REGS_PARM2(ctx);

    if (!ptr)
        return 0;

    // Look up the allocation to get the original stack
    struct alloc_info *info = bpf_map_lookup_elem(&live_allocs, &ptr);
    u64 key_addr = 0;
    u64 alloc_size = size;

    if (info && info->stack_len > 0) {
        key_addr = info->stack[0];  // Use first frame as key
        alloc_size = info->size;
        bpf_map_delete_elem(&live_allocs, &ptr);
    }

    // Skip if we don't have a valid key
    if (key_addr == 0)
        return 0;

    // Update stats
    struct heap_stats *stats = bpf_map_lookup_elem(&heap_stats, &key_addr);
    if (stats) {
        __sync_fetch_and_sub(&stats->live_bytes, alloc_size);
        __sync_fetch_and_add(&stats->total_frees, 1);
        __sync_fetch_and_add(&stats->total_free_bytes, alloc_size);
    }

    // Send event to userspace
    struct heap_event *event = bpf_ringbuf_reserve(&events, sizeof(*event), 0);
    if (event) {
        event->user_addr = key_addr;
        event->ptr = ptr;
        event->size = -(s64)alloc_size;
        event->event_type = 1;  // free
        bpf_ringbuf_submit(event, 0);
    }

    return 0;
}

// __rust_realloc(ptr: *mut u8, old_size: usize, align: usize, new_size: usize) -> *mut u8
SEC("uprobe/rust_realloc")
int uprobe_rust_realloc_v2(struct pt_regs *ctx) {
    u32 pid = bpf_get_current_pid_tgid() >> 32;
    u32 target_pid = get_target_pid();
    if (target_pid && pid != target_pid)
        return 0;

    u64 old_ptr = PT_REGS_PARM1(ctx);
    u64 old_size = PT_REGS_PARM2(ctx);
    u64 new_size = PT_REGS_PARM4(ctx);

    struct realloc_info ri = {
        .old_ptr = old_ptr,
        .old_size = old_size,
        .new_size = new_size,
        .stack_len = 0,
    };

    // Walk frame pointers to capture inline stack
    u64 fp = PT_REGS_FP(ctx);
    #pragma unroll
    for (int i = 0; i < INLINE_STACK_DEPTH && fp != 0; i++) {
        u64 ret_addr = 0;
        if (bpf_probe_read_user(&ret_addr, sizeof(ret_addr), (void *)(fp + 8)) != 0)
            break;
        if (ret_addr != 0) {
            ri.stack[ri.stack_len] = ret_addr;
            ri.stack_len++;
        }
        u64 next_fp = 0;
        if (bpf_probe_read_user(&next_fp, sizeof(next_fp), (void *)fp) != 0)
            break;
        if (next_fp <= fp)
            break;
        fp = next_fp;
    }

    if (ri.stack_len == 0) {
        u64 ret_addr = 0;
        bpf_probe_read_user(&ret_addr, sizeof(ret_addr), (void *)PT_REGS_SP(ctx));
        if (ret_addr != 0) {
            ri.stack[0] = ret_addr;
            ri.stack_len = 1;
        }
    }

    u32 zero = 0;
    bpf_map_update_elem(&realloc_scratch, &zero, &ri, BPF_ANY);

    return 0;
}

SEC("uretprobe/rust_realloc")
int uretprobe_rust_realloc(struct pt_regs *ctx) {
    u32 pid = bpf_get_current_pid_tgid() >> 32;
    u32 target_pid = get_target_pid();
    if (target_pid && pid != target_pid)
        return 0;

    u64 new_ptr = PT_REGS_RC(ctx);

    u32 zero = 0;
    struct realloc_info *ri = bpf_map_lookup_elem(&realloc_scratch, &zero);
    if (!ri || ri->stack_len == 0)
        return 0;

    u64 old_ptr = ri->old_ptr;
    u64 old_size = ri->old_size;
    u64 new_size = ri->new_size;
    u64 key_addr = ri->stack[0];

    // Get stack from stored allocation if available (for proper attribution)
    struct alloc_info *old_info = bpf_map_lookup_elem(&live_allocs, &old_ptr);
    if (old_info && old_info->stack_len > 0) {
        key_addr = old_info->stack[0];
        old_size = old_info->size;
    }

    // Remove old allocation
    if (old_ptr) {
        bpf_map_delete_elem(&live_allocs, &old_ptr);
    }

    // Add new allocation (if successful)
    if (new_ptr) {
        struct alloc_info new_info = {
            .size = new_size,
            .stack_len = ri->stack_len,
        };
        #pragma unroll
        for (int i = 0; i < INLINE_STACK_DEPTH; i++) {
            new_info.stack[i] = (i < ri->stack_len) ? ri->stack[i] : 0;
        }
        bpf_map_update_elem(&live_allocs, &new_ptr, &new_info, BPF_ANY);
    }

    // Update stats: size change = new_size - old_size
    s64 delta = (s64)new_size - (s64)old_size;

    struct heap_stats *stats = bpf_map_lookup_elem(&heap_stats, &key_addr);
    if (stats) {
        __sync_fetch_and_add(&stats->live_bytes, delta);
        if (delta > 0) {
            __sync_fetch_and_add(&stats->total_alloc_bytes, delta);
        } else {
            __sync_fetch_and_add(&stats->total_free_bytes, -delta);
        }
    } else if (new_ptr) {
        struct heap_stats new_stats = {
            .live_bytes = new_size,
            .total_allocs = 1,
            .total_frees = 0,
            .total_alloc_bytes = new_size,
            .total_free_bytes = 0,
        };
        bpf_map_update_elem(&heap_stats, &key_addr, &new_stats, BPF_NOEXIST);
    }

    // Send event
    struct heap_event *event = bpf_ringbuf_reserve(&events, sizeof(*event), 0);
    if (event) {
        event->user_addr = key_addr;
        event->ptr = new_ptr ? new_ptr : old_ptr;
        event->size = delta;
        event->event_type = 2;  // realloc
        bpf_ringbuf_submit(event, 0);
    }

    return 0;
}

char LICENSE[] SEC("license") = "Dual MIT/GPL";
