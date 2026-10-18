# TriOS Memory Architecture & Physical/Virtual Memory Map

TriOS implements an enterprise-grade virtual and physical memory management architecture for 64-bit x86_64 systems using standard 4-level paging (PML4 -> PDPT -> PD -> PT).

---

## 1. Physical Memory Management

Physical memory is tracked using a **Bitmap Frame Allocator** located in [`kernel/src/memory/bitmap.rs`](file:///d:/trios/kernel/src/memory/bitmap.rs).

- **Frame Size**: 4096 bytes (4 KiB standard x86_64 page).
- **Tracking Structure**: Static bit array where each bit represents a single physical frame:
  - `0`: Available / free frame.
  - `1`: In-use / allocated frame.
- **Hardware Map Acquisition**: During early UEFI boot, the firmware memory map (`bootloader::boot_info::MemoryRegions`) is parsed:
  - Usable memory regions are marked free in the bitmap.
  - Reserved, ACPI reclaimable, and kernel-reserved physical regions are preserved.
- **Allocation Primitives**:
  - `alloc_frame() -> Option<PhysFrame>`: First-fit allocation returning the lowest available frame.
  - `free_frame(frame: PhysFrame)`: Returns frame to available pool.

---

## 2. Virtual Memory Map (x86_64 4-Level Paging)

x86_64 processors enforce canonical address ranges where bits 48 through 63 must match bit 47. This divides the 64-bit virtual address space into lower-half user space and upper-half kernel space:

```
+-------------------------------------------------------------------------+
| Virtual Address Range                     | Purpose & Permissions       |
+-------------------------------------------------------------------------+
| 0x0000_0000_0000_0000 - 0x0000_007F_FFFF_FFFF | Canonical Lower Half (User) |
|   0x0000_0000_0040_0000                  | User ELF Image (Code/Data)  |
|   0x0000_0000_1000_0000                  | User mmap Virtual Heap Base |
|   0x0000_0000_7FFE_C000 - 0x7FFF_0000    | User Stack (16 KiB default) |
|   0x0000_0000_7FFE_B000                  | Stack Bottom Guard Page     |
+-------------------------------------------------------------------------+
| 0x0000_0080_0000_0000 - 0xFFFF_7FFF_FFFF_FFFF | Non-Canonical (Hardware Fault) |
+-------------------------------------------------------------------------+
| 0xFFFF_8000_0000_0000 - 0xFFFF_FFFF_FFFF_FFFF | Canonical Upper Half (Kernel)|
|   0x0000_0200_0000_0000 (phys_offset)    | Physical Memory Direct Map  |
|   0x4444_4444_0000 - 0x4444_4644_0000    | 32 MiB Kernel Heap (Slabs)  |
|   MMIO Regions (Dynamic)                 | E1000 BAR0, AHCI BAR5, APIC |
+-------------------------------------------------------------------------+
```

---

## 3. Kernel Memory Subsystems

### 3.1 Kernel Heap & Slab Allocator
The kernel heap is defined in [`kernel/src/allocator.rs`](file:///d:/trios/kernel/src/allocator.rs):
- **Total Size**: 32 MiB (`0x200_0000` bytes).
- **Base Address**: `0x4444_4444_0000`.
- **Hybrid Allocator Design**:
  1. **Slab Allocator**: Fixed-size bins for small allocations to eliminate internal fragmentation:
     - 32-byte slabs
     - 64-byte slabs
     - 128-byte slabs
     - 256-byte slabs
     - 512-byte slabs
     - 1024-byte slabs
     - 2048-byte slabs
  2. **Linked-List Allocator**: Fallback for allocations larger than 2048 bytes.

### 3.2 Physical Direct Map (`phys_offset`)
The bootloader maps all physical memory at a fixed virtual offset (typically `0x0000_0200_0000_0000`). This permits kernel routines to access raw physical frames without modifying active page tables by adding `phys_offset`.

---

## 4. User Space Isolation & Security

User mode (Ring 3) processes execute in their own isolated PML4 page tables created during `spawn_process()`:
- **Kernel Space Sharing**: PML4 entries `1..512` are cloned from the kernel PML4 to permit kernel interrupt and syscall servicing without page table switches.
- **User Space Isolation**: PML4 entry `0` (`0x0000_0000_0000_0000 .. 0x0000_007F_FFFF_FFFF`) is unique per process.
- **User Pointer Validation (`validate_user_buffer`)**:
  - Rejects null pointers.
  - Rejects pointers crossing into the canonical upper half (`>= 0x0000_0080_0000_0000`).
  - Walks L4, L3, L2, L1 page tables to ensure pages are present and marked `USER_ACCESSIBLE`.
  - Enforces `WRITABLE` permission when destination buffers are written to by the kernel.
