//! Ring 3 Process Representation, Process Table, and Per-Process PML4 Address Space.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::{
    VirtAddr,
    instructions::tlb,
    registers::control::Cr3,
    structures::paging::{PageTable, PageTableFlags, PhysFrame, Size4KiB},
};

use crate::memory::{bitmap, virt::PageFlags};

/// Process ID type.
pub type ProcessId = u64;

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// State of a user process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ProcessState {
    Ready,
    Running,
    Sleeping(u64),
    Dead,
}

/// An open file descriptor entry for a process.
#[derive(Debug, Clone)]
pub enum FdKind {
    Stdin,
    Stdout,
    Stderr,
    File { path: String, offset: usize, writable: bool },
    PipeRead { pipe_id: usize },
    PipeWrite { pipe_id: usize },
    Socket { sock_id: usize },
}

#[derive(Debug, Clone)]
pub struct ProcessFileDesc {
    pub kind: FdKind,
}

pub struct PipeBuffer {
    pub data: alloc::collections::VecDeque<u8>,
    pub reader_count: usize,
    pub writer_count: usize,
}

pub static PIPES: Mutex<Vec<Option<PipeBuffer>>> = Mutex::new(Vec::new());

pub struct PtyBuffer {
    pub input_queue: alloc::collections::VecDeque<u8>,
    pub output_queue: alloc::collections::VecDeque<u8>,
}

pub static SHELL_PTY: Mutex<PtyBuffer> = Mutex::new(PtyBuffer {
    input_queue: alloc::collections::VecDeque::new(),
    output_queue: alloc::collections::VecDeque::new(),
});

/// A Ring 3 user process with its own PML4 address space.
pub struct Process {
    pub pid: ProcessId,
    pub parent_pid: Option<ProcessId>,
    pub name: String,
    pub state: ProcessState,
    /// Physical frame of this process's PML4 page table.
    pub pml4_frame: PhysFrame<Size4KiB>,
    /// All physical frames allocated to this process in user space (for teardown cleanup).
    pub allocated_frames: Vec<PhysFrame<Size4KiB>>,
    /// Per-process open file descriptor table.
    pub fd_table: Vec<Option<ProcessFileDesc>>,
    /// Current working directory.
    pub cwd: String,
    /// Next virtual address for user `mmap` allocations.
    pub mmap_next: u64,
    /// Exit code when process terminates.
    pub exit_code: i32,
    /// User instruction pointer (entry point or saved RIP).
    pub user_rip: u64,
    /// User stack pointer.
    pub user_rsp: u64,
    /// User argc passed to entry point.
    pub user_argc: u64,
    /// User argv passed to entry point.
    pub user_argv: u64,
}

impl Process {
    /// Creates a new user process with an isolated PML4 table whose upper half (kernel)
    /// is shared with the kernel's active page table.
    pub fn new(name: &str, user_entry: u64) -> Result<Box<Self>, &'static str> {
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        let phys_offset = crate::memory::get_phys_offset().ok_or("phys offset missing")?;

        // 1. Allocate a physical frame for the new PML4
        let pml4_frame = bitmap::alloc_frame().ok_or("out of physical frames for process PML4")?;

        // 2. Clone active kernel PML4 entries (1..512) so kernel direct map, code, stack, and devices remain fully mapped
        unsafe {
            let kernel_l4_virt = phys_offset + Cr3::read().0.start_address().as_u64();
            let kernel_l4 = &*kernel_l4_virt.as_ptr::<PageTable>();

            let new_l4_virt = phys_offset + pml4_frame.start_address().as_u64();
            let new_l4 = &mut *new_l4_virt.as_mut_ptr::<PageTable>();

            // Entry 0 is isolated user space (0..512 GiB)
            new_l4[0].set_unused();

            // Entries 1..512 are shared kernel space (kernel image, direct map, stack, GOP, etc.)
            for i in 1..512 {
                new_l4[i] = kernel_l4[i].clone();
            }
        }

        let mut proc = Box::new(Self {
            pid,
            parent_pid: None,
            name: String::from(name),
            state: ProcessState::Ready,
            pml4_frame,
            allocated_frames: Vec::new(),
            fd_table: vec![
                Some(ProcessFileDesc { kind: FdKind::Stdin }),  // fd 0 = stdin
                Some(ProcessFileDesc { kind: FdKind::Stdout }), // fd 1 = stdout
                Some(ProcessFileDesc { kind: FdKind::Stderr }), // fd 2 = stderr
            ],
            cwd: String::from("/"),
            mmap_next: 0x0000_0000_1000_0000,
            exit_code: 0,
            user_rip: user_entry,
            user_rsp: 0x0000_0000_7FFF_0000,
            user_argc: 0,
            user_argv: 0,
        });

        // Map initial 16 KiB user stack at top of user 2GB space (0x7FFE_C000..0x7FFF_0000)
        proc.map_user_stack(4)?;

        Ok(proc)
    }

    /// Maps `pages` pages for the user stack ending at `0x0000_0000_7FFF_0000`.
    fn map_user_stack(&mut self, pages: usize) -> Result<(), &'static str> {
        let stack_bottom = 0x0000_0000_7FFF_0000u64 - (pages as u64 * 4096);
        for i in 0..pages {
            let virt = VirtAddr::new(stack_bottom + (i as u64 * 4096));
            let frame = bitmap::alloc_frame().ok_or("out of frames for user stack")?;

            // Map in process's PML4
            map_in_pml4(self.pml4_frame, virt, frame, PageFlags::USER_DATA)?;
            self.allocated_frames.push(frame);

            // Zero out user stack page
            let phys_offset = crate::memory::get_phys_offset().unwrap();
            unsafe {
                let ptr = (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
                core::ptr::write_bytes(ptr, 0, 4096);
            }
        }
        Ok(())
    }

    /// Allocates and maps virtual user pages via `mmap`.
    pub fn mmap(&mut self, len: usize) -> Result<u64, &'static str> {
        if len == 0 || len > 64 * 1024 * 1024 {
            return Err("invalid mmap length (exceeds 64 MiB limit or zero)");
        }
        let page_count = (len + 4095) / 4096;
        let bytes_needed = (page_count as u64) * 4096;
        if self.mmap_next.checked_add(bytes_needed).map_or(true, |end| end > 0x0000_0080_0000_0000) {
            return Err("mmap out of user virtual address space");
        }
        let start_addr = self.mmap_next;
        self.mmap_next += bytes_needed;

        for i in 0..page_count {
            let virt = VirtAddr::new(start_addr + (i as u64 * 4096));
            let frame = bitmap::alloc_frame().ok_or("out of frames for mmap")?;
            map_in_pml4(self.pml4_frame, virt, frame, PageFlags::USER_DATA)?;
            self.allocated_frames.push(frame);

            let phys_offset = crate::memory::get_phys_offset().unwrap();
            unsafe {
                let ptr = (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
                core::ptr::write_bytes(ptr, 0, 4096);
            }
        }

        Ok(start_addr)
    }

    /// Cleans up all physical frames, user page tables, and resources on process exit.
    pub fn teardown(&mut self) {
        serial_println!(
            "[process] tearing down PID #{} ('{}') — freeing {} user frames",
            self.pid,
            self.name,
            self.allocated_frames.len()
        );

        // 1. Free all tracked user physical frames
        for frame in self.allocated_frames.drain(..) {
            bitmap::free_frame(frame);
        }

        // 2. Free user-space page directory frames (lower half entries of PML4)
        let phys_offset = match crate::memory::get_phys_offset() {
            Some(o) => o,
            None => return,
        };

        unsafe {
            let l4_virt = phys_offset + self.pml4_frame.start_address().as_u64();
            let l4 = &mut *l4_virt.as_mut_ptr::<PageTable>();

            let l4_entry = &mut l4[0];
            if !l4_entry.is_unused() && !l4_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                let l3_frame = PhysFrame::containing_address(l4_entry.addr());
                let l3_virt = phys_offset + l3_frame.start_address().as_u64();
                let l3 = &mut *l3_virt.as_mut_ptr::<PageTable>();

                for p3_idx in 0..512 {
                    let l3_entry = &mut l3[p3_idx];
                    if !l3_entry.is_unused() && !l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                        let l2_frame = PhysFrame::containing_address(l3_entry.addr());
                        let l2_virt = phys_offset + l2_frame.start_address().as_u64();
                        let l2 = &mut *l2_virt.as_mut_ptr::<PageTable>();

                        for p2_idx in 0..512 {
                            let l2_entry = &mut l2[p2_idx];
                            if !l2_entry.is_unused() && !l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                                let l1_frame = PhysFrame::containing_address(l2_entry.addr());
                                bitmap::free_frame(l1_frame);
                            }
                        }
                        bitmap::free_frame(l2_frame);
                    }
                }
                bitmap::free_frame(l3_frame);
            }
            l4_entry.set_unused();
        }

        // 3. Close open file descriptors and release pipe references
        for desc_opt in self.fd_table.drain(..) {
            if let Some(desc) = desc_opt {
                match desc.kind {
                    FdKind::PipeRead { pipe_id } => {
                        let mut pipes = PIPES.lock();
                        if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                            pipe.reader_count = pipe.reader_count.saturating_sub(1);
                            if pipe.reader_count == 0 && pipe.writer_count == 0 {
                                pipes[pipe_id] = None;
                            }
                        }
                    }
                    FdKind::PipeWrite { pipe_id } => {
                        let mut pipes = PIPES.lock();
                        if let Some(Some(pipe)) = pipes.get_mut(pipe_id) {
                            pipe.writer_count = pipe.writer_count.saturating_sub(1);
                            if pipe.reader_count == 0 && pipe.writer_count == 0 {
                                pipes[pipe_id] = None;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // 4. Free the PML4 table itself
        bitmap::free_frame(self.pml4_frame);
        self.state = ProcessState::Dead;
    }
}

/// Maps a virtual page inside a specific PML4 table.
pub fn map_in_pml4(
    pml4_frame: PhysFrame<Size4KiB>,
    virt: VirtAddr,
    frame: PhysFrame<Size4KiB>,
    flags: PageFlags,
) -> Result<(), &'static str> {
    let phys_offset = crate::memory::get_phys_offset().ok_or("phys offset missing")?;

    let l4_virt = phys_offset + pml4_frame.start_address().as_u64();
    let l4 = unsafe { &mut *l4_virt.as_mut_ptr::<PageTable>() };

    let p4_idx = virt.p4_index();
    let l4_entry = &mut l4[p4_idx];

    if l4_entry.is_unused() {
        let new_frame = bitmap::alloc_frame().ok_or("out of physical frames for L3")?;
        unsafe {
            let ptr = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            core::ptr::write_bytes(ptr, 0, 1);
        }
        l4_entry.set_addr(
            new_frame.start_address(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    let l3_addr = phys_offset + l4_entry.addr().as_u64();
    let l3 = unsafe { &mut *l3_addr.as_mut_ptr::<PageTable>() };
    let p3_idx = virt.p3_index();
    let l3_entry = &mut l3[p3_idx];

    if l3_entry.is_unused() {
        let new_frame = bitmap::alloc_frame().ok_or("out of physical frames for L2")?;
        unsafe {
            let ptr = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            core::ptr::write_bytes(ptr, 0, 1);
        }
        l3_entry.set_addr(
            new_frame.start_address(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    let l2_addr = phys_offset + l3_entry.addr().as_u64();
    let l2 = unsafe { &mut *l2_addr.as_mut_ptr::<PageTable>() };
    let p2_idx = virt.p2_index();
    let l2_entry = &mut l2[p2_idx];

    if l2_entry.is_unused() {
        let new_frame = bitmap::alloc_frame().ok_or("out of physical frames for L1")?;
        unsafe {
            let ptr = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            core::ptr::write_bytes(ptr, 0, 1);
        }
        l2_entry.set_addr(
            new_frame.start_address(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    let l1_addr = phys_offset + l2_entry.addr().as_u64();
    let l1 = unsafe { &mut *l1_addr.as_mut_ptr::<PageTable>() };
    let p1_idx = virt.p1_index();
    let l1_entry = &mut l1[p1_idx];

    l1_entry.set_addr(frame.start_address(), flags.to_x86_flags());
    tlb::flush(virt);

    Ok(())
}

/// Global Process Table.
pub static PROCESS_TABLE: Mutex<Vec<Box<Process>>> = Mutex::new(Vec::new());

/// Global Current Process ID.
static CURRENT_PID: AtomicU64 = AtomicU64::new(0);

/// Sets the currently active running PID.
pub fn set_current_pid(pid: ProcessId) {
    CURRENT_PID.store(pid, Ordering::SeqCst);
}

/// Returns the currently active running PID (0 if kernel).
pub fn current_pid() -> ProcessId {
    CURRENT_PID.load(Ordering::Relaxed)
}

/// Registers a new process in the process table.
pub fn register_process(proc: Box<Process>) -> ProcessId {
    let pid = proc.pid;
    PROCESS_TABLE.lock().push(proc);
    pid
}

/// Terminates and tears down a process by PID.
pub fn kill_process(pid: ProcessId) -> bool {
    let mut table = PROCESS_TABLE.lock();
    if let Some(pos) = table.iter().position(|p| p.pid == pid) {
        let mut proc = table.remove(pos);
        proc.teardown();
        true
    } else {
        false
    }
}

/// Kernel baseline PML4 frame (used to restore address space upon process exit/fault).
static KERNEL_PML4_FRAME: spin::Mutex<Option<PhysFrame<Size4KiB>>> = spin::Mutex::new(None);

/// Stores the kernel baseline PML4 frame from boot.
pub fn init_kernel_pml4(frame: PhysFrame<Size4KiB>) {
    *KERNEL_PML4_FRAME.lock() = Some(frame);
}

/// Restores the kernel baseline PML4 frame to CR3.
pub fn restore_kernel_pml4() {
    if let Some(frame) = *KERNEL_PML4_FRAME.lock() {
        unsafe {
            Cr3::write(frame, x86_64::registers::control::Cr3Flags::empty());
        }
    }
}

/// Validates that a user-supplied pointer and buffer range is strictly within user space,
/// currently mapped in the given PML4, and possesses the required permissions (user flag & write if requested).
pub fn validate_user_buffer(
    pml4_frame: PhysFrame<Size4KiB>,
    ptr: usize,
    len: usize,
    need_write: bool,
) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("null pointer dereference rejected");
    }
    let end = ptr
        .checked_add(len)
        .ok_or("user buffer pointer arithmetic overflow")?;

    // Must be in canonical user space (PML4 entry 0, < 0x0000_0080_0000_0000)
    if end > 0x0000_0080_0000_0000 {
        return Err("pointer resides outside canonical user space");
    }

    let phys_offset = crate::memory::get_phys_offset().ok_or("phys offset missing")?;
    let l4_virt = phys_offset + pml4_frame.start_address().as_u64();
    let l4 = unsafe { &*l4_virt.as_ptr::<PageTable>() };

    let mut curr_page = ptr & !0xFFF;
    while curr_page < end {
        let virt = VirtAddr::new(curr_page as u64);

        let l4_entry = &l4[virt.p4_index()];
        if l4_entry.is_unused() || !l4_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
            return Err("page table L4 entry invalid or not user-accessible");
        }

        let l3_virt = phys_offset + l4_entry.addr().as_u64();
        let l3 = unsafe { &*l3_virt.as_ptr::<PageTable>() };
        let l3_entry = &l3[virt.p3_index()];
        if l3_entry.is_unused() || !l3_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
            return Err("page table L3 entry invalid or not user-accessible");
        }

        if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            curr_page += 1024 * 1024 * 1024;
            continue;
        }

        let l2_virt = phys_offset + l3_entry.addr().as_u64();
        let l2 = unsafe { &*l2_virt.as_ptr::<PageTable>() };
        let l2_entry = &l2[virt.p2_index()];
        if l2_entry.is_unused() || !l2_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
            return Err("page table L2 entry invalid or not user-accessible");
        }

        if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            curr_page += 2 * 1024 * 1024;
            continue;
        }

        let l1_virt = phys_offset + l2_entry.addr().as_u64();
        let l1 = unsafe { &*l1_virt.as_ptr::<PageTable>() };
        let l1_entry = &l1[virt.p1_index()];
        if l1_entry.is_unused() || !l1_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
            return Err("page table L1 entry invalid or not user-accessible");
        }

        if need_write && !l1_entry.flags().contains(PageTableFlags::WRITABLE) {
            return Err("user buffer page is read-only but write permission required");
        }

        curr_page += 4096;
    }

    Ok(())
}

/// Execution trampoline to jump into Ring 3 user mode.
pub extern "C" fn run_process_ring3(pid: u64) {
    let (entry, stack, argc, argv, pml4) = {
        let mut table = PROCESS_TABLE.lock();
        let proc = match table.iter_mut().find(|p| p.pid == pid) {
            Some(p) => p,
            None => return,
        };
        proc.state = ProcessState::Running;
        set_current_pid(pid);
        (proc.user_rip, proc.user_rsp, proc.user_argc, proc.user_argv, proc.pml4_frame)
    };

    serial_println!(
        "[ring3  ] jumping to Ring 3 userspace: PID #{pid}, entry={entry:#x}, stack={stack:#x}, argc={argc}, argv={argv:#x}, CR3={:#x}",
        pml4.start_address().as_u64()
    );

    let kstack_top = {
        let lock = crate::sched::SCHEDULER.lock();
        lock.as_ref()
            .and_then(|s| s.current.kstack.as_ref().map(|k| k.stack_top.as_u64()))
            .unwrap_or_else(|| crate::gdt::kernel_rsp0_stack_top())
    };
    crate::syscall::set_kernel_syscall_rsp(kstack_top);

    // Switch to the process's isolated PML4 table
    unsafe {
        Cr3::write(pml4, x86_64::registers::control::Cr3Flags::empty());
        enter_user_mode(entry, stack, argc, argv);
    }
}

/// Low-level assembly function to switch CPL 0 -> CPL 3 via `iretq`.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user_mode(entry: u64, stack: u64, argc: u64, argv: u64) -> ! {
    core::arch::naked_asm!(
        // RDI = entry (user RIP)
        // RSI = stack (user RSP)
        // RDX = argc
        // RCX = argv
        "mov ax, 0x1B",
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",

        // Build iretq frame on current stack:
        // SS, RSP, RFLAGS, CS, RIP
        "push 0x1B",         // SS (User Data RPL 3)
        "push rsi",          // RSP (User stack)
        "push 0x202",        // RFLAGS (IF=1)
        "push 0x23",         // CS (User Code RPL 3)
        "push rdi",          // RIP (User entry point)

        // Set SysV ABI registers for user entry point:
        // RDI = argc
        // RSI = argv
        "mov rdi, rdx",
        "mov rsi, rcx",
        "xor edx, edx",
        "xor ecx, ecx",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",
        "iretq"
    );
}

extern "C" fn user_process_entry(pid_arg: usize) {
    let pid = pid_arg as u64;
    if pid != 0 {
        run_process_ring3(pid);
    }
}

/// Spawns a new user process from raw ELF binary bytes with string arguments and parent tracking.
pub fn spawn_process_with_args(
    path: &str,
    elf_bytes: &[u8],
    args: &[String],
    parent_pid: Option<ProcessId>,
) -> Result<ProcessId, &'static str> {
    let hdr = crate::fs::elf::parse_header(elf_bytes).map_err(|_| "invalid ELF binary")?;
    let mut proc = Process::new(path, hdr.entry_point)?;
    proc.parent_pid = parent_pid;

    // Inherit cwd and non-standard file descriptors from parent if present
    if let Some(ppid) = parent_pid {
        let table = PROCESS_TABLE.lock();
        if let Some(parent) = table.iter().find(|p| p.pid == ppid) {
            proc.cwd = parent.cwd.clone();
            for (idx, fd_opt) in parent.fd_table.iter().enumerate() {
                if idx < proc.fd_table.len() {
                    proc.fd_table[idx] = fd_opt.clone();
                } else if let Some(fd) = fd_opt {
                    while proc.fd_table.len() < idx {
                        proc.fd_table.push(None);
                    }
                    proc.fd_table.push(Some(fd.clone()));
                }
            }
        }
    }

    crate::fs::elf::load_elf_into_process(elf_bytes, &mut proc)
        .map_err(|_| "failed to load ELF segments into process PML4")?;

    let phys_offset = crate::memory::get_phys_offset().ok_or("phys offset missing")?;

    // The top stack frame (0x7FFE_F000..0x7FFF_0000) is the 4th frame in allocated_frames
    let top_frame = proc.allocated_frames[3];
    let top_frame_virt = phys_offset + top_frame.start_address().as_u64();
    let top_frame_ptr = top_frame_virt.as_mut_ptr::<u8>();

    let mut curr_offset = 4096usize;
    let mut arg_ptrs_virt = Vec::new();

    // Copy argument strings into top of user stack frame
    for arg in args {
        let bytes = arg.as_bytes();
        let needed = bytes.len() + 1;
        if curr_offset < needed + 256 {
            break;
        }
        curr_offset -= needed;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), top_frame_ptr.add(curr_offset), bytes.len());
            *top_frame_ptr.add(curr_offset + bytes.len()) = 0;
        }
        let user_str_vaddr = 0x0000_0000_7FFE_F000u64 + curr_offset as u64;
        arg_ptrs_virt.push(user_str_vaddr);
    }

    // Align down to 8 bytes for pointer array
    curr_offset &= !7;

    // Push null pointer (argv[argc] = NULL)
    curr_offset -= 8;
    unsafe {
        *(top_frame_ptr.add(curr_offset) as *mut u64) = 0;
    }

    // Push argv pointers in reverse order so argv[0] is at lowest address
    for &ptr_virt in arg_ptrs_virt.iter().rev() {
        curr_offset -= 8;
        unsafe {
            *(top_frame_ptr.add(curr_offset) as *mut u64) = ptr_virt;
        }
    }

    let argv_user_addr = 0x0000_0000_7FFE_F000u64 + curr_offset as u64;

    // Align user stack pointer down to 16 bytes (SysV ABI constraint)
    curr_offset &= !15;
    let user_rsp = 0x0000_0000_7FFE_F000u64 + curr_offset as u64;

    proc.user_rsp = user_rsp;
    proc.user_argc = arg_ptrs_virt.len() as u64;
    proc.user_argv = argv_user_addr;

    let pid = register_process(proc);
    let _tid = crate::sched::spawn_process_thread(path, user_process_entry, pid);

    Ok(pid)
}

/// Spawns a new user process from raw ELF binary bytes.
pub fn spawn_process(
    name: &str,
    elf_bytes: &[u8],
) -> Result<(ProcessId, crate::sched::ThreadId), &'static str> {
    let arg = String::from(name);
    let pid = spawn_process_with_args(name, elf_bytes, &[arg], None)?;
    Ok((pid, 0))
}
