# TriOS System Call Architecture & Syscall Reference

TriOS implements hardware-accelerated fast system calls via the x86_64 `syscall` and `sysretq` instructions.

---

## 1. Hardware Configuration & Entry Mechanics

Fast system call switching is initialized in [`kernel/src/syscall/mod.rs`](file:///d:/trios/kernel/src/syscall/mod.rs):
- **`IA32_EFER.SCE` (Bit 0)**: Set to 1 to enable the `syscall`/`sysret` instructions.
- **`IA32_STAR` MSR (`0xC0000081`)**:
  - Bits 32..47: Kernel Code/Data selector base (`0x0008`).
  - Bits 48..63: User Code/Data selector base (`0x0010`).
- **`IA32_LSTAR` MSR (`0xC0000082`)**: Programmed with the virtual address of [`syscall_entry`](file:///d:/trios/kernel/src/syscall/mod.rs#L38).
- **`IA32_FMASK` MSR (`0xC0000084`)**: Programmed with `0x200` to automatically clear RFLAGS.IF (disable hardware interrupts) on syscall entry.

### Register Calling Convention

When a user program issues `syscall`:
- `RAX`: System Call Number.
- `RDI`: Argument 1.
- `RSI`: Argument 2.
- `RDX`: Argument 3.
- `R10`: Argument 4 (kernel moves from RCX/R10).
- `RCX`: Overwritten by hardware with User RIP.
- `R11`: Overwritten by hardware with User RFLAGS.
- `RAX`: Returns syscall result (`0` or positive on success, `usize::MAX` on error / `-EFAULT`).

Preserved registers across calls: `RBX`, `RBP`, `RSP`, `R12`, `R13`, `R14`, `R15`.

---

## 2. Syscall Dispatch Table

| Number | Name | Arguments | Return Value | Description |
|---|---|---|---|---|
| `0` | `SYS_EXIT` | `arg1`: exit_code (`i32`) | Does not return | Terminates the calling process, cleans up PML4 and user frames, schedules next thread. |
| `1` | `SYS_WRITE` | `arg1`: fd (`usize`)<br>`arg2`: buf (`*const u8`)<br>`arg3`: len (`usize`) | Bytes written, or `usize::MAX` | Writes bytes to standard streams (`1`=stdout, `2`=stderr) or open VFS file descriptors (`>=3`). |
| `2` | `SYS_READ` | `arg1`: fd (`usize`)<br>`arg2`: buf (`*mut u8`)<br>`arg3`: len (`usize`) | Bytes read, or `usize::MAX` | Reads bytes from open VFS file descriptors (`>=3`). Requires destination buffer to be writable user memory. |
| `3` | `SYS_YIELD` | None | `0` | Voluntarily yields remaining CPU timeslice to the scheduler. |
| `4` | `SYS_UPTIME` | None | Seconds (`usize`) | Returns total uptime in seconds since UEFI kernel boot. |
| `5` | `SYS_GETPID` | None | PID (`usize`) | Returns the process ID of the active user process. |
| `6` | `SYS_SLEEP_MS` | `arg1`: milliseconds (`usize`) | `0` | Suspends execution of the calling process for specified milliseconds. |
| `7` | `SYS_MMAP` | `arg1`: addr hint (`usize`)<br>`arg2`: len (`usize`)<br>`arg3`: prot (`usize`) | Virtual address (`usize`) or `0` | Allocates and maps anonymous virtual user pages in process PML4. |
| `8` | `SYS_MUNMAP` | `arg1`: addr (`usize`)<br>`arg2`: len (`usize`) | `0` | Unmaps previously mapped user virtual pages. |
| `9` | `SYS_OPEN` | `arg1`: path (`*const u8`)<br>`arg2`: flags (`0`=read, `1`=write) | FD (`usize`) or `usize::MAX` | Opens a file from ramfs (`/`) or disk (`/disk`). Returns allocated file descriptor. |
| `10` | `SYS_CLOSE` | `arg1`: fd (`usize`) | `0` on success, `usize::MAX` | Closes an active file descriptor (`>=3`). |

---

## 3. Strict Pointer Validation & Fault Isolation

The kernel isolates user space and protects kernel integrity via [`validate_user_buffer`](file:///d:/trios/kernel/src/sched/process.rs#L352):
1. **Null Check**: Buffers starting at address `0` are rejected immediately.
2. **Canonical User Bound**: Any pointer where `ptr + len > 0x0000_0080_0000_0000` is rejected with `-EFAULT`.
3. **Overflow Protection**: `ptr.checked_add(len)` prevents integer wrap-around attacks.
4. **Paging Permission Walk**: The kernel walks the process's L4, L3, L2, and L1 page table entries to verify the `USER_ACCESSIBLE` bit is set for every 4 KiB page.
5. **Write Permission Enforcement**: For `SYS_READ`, every destination page is validated to have `WRITABLE` set, preventing user processes from corrupting read-only code segments.
