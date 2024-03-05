use crate::logger::LoggerInner;
use core::arch::asm;
use core::mem::MaybeUninit;
use core::ops::Range;
use core::ptr::addr_of_mut;
use core::{mem, slice};
use dtb_parser::{DevTree, Error, Node, Strings, Visitor};
use spin::Once;

const STACK_SIZE_PAGES: usize = 16;
const PAGE_SIZE: usize = 4096;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!(
    ".option push",
    ".option norelax",
    "    la		gp, __global_pointer$",
    ".option pop",
    "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
    "li     t0, {stack_size}", // load the stack size
    "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
    "mul    t0, t0, t1", // multiply the stack size by the hart id to get the offset
    "add    sp, sp, t0", // add the offset from sp to get the harts stack pointer

    // "addi sp, sp, -{trap_frame_size}",
    // "csrrw x0, sscratch, sp", // sscratch points to the trap frame

    "jal zero, {start_rust}", // jump into Rust

    stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
    // trap_frame_size = const mem::size_of::<TrapFrame>(),
    start_rust = sym start,
    options(noreturn)
    )
}

unsafe extern "C" fn start(hartid: usize, opaque: *const u8) -> ! {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        extern "C" {
            static mut __bss_start: u64;
            static mut __bss_end: u64;
        }

        // Zero BSS section
        let mut ptr = addr_of_mut!(__bss_start);
        let end = addr_of_mut!(__bss_end);
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }

        crate::logger::init();

        unsafe {
            let mut w = LoggerInner;
            let mut dbg = dtb_parser::debug::DebugVisitor::new(&mut w);

            DevTree::from_raw(opaque).unwrap().visit(&mut dbg).unwrap()
        }

        MachineInfo::from_dtb(opaque);

        // for hart in 0..8 {
        //     if hart != hartid {
        //         sbicall::hsm::start_hart(hart, _start as usize, 0).unwrap();
        //     }
        // }
    });

    crate::main(hartid)
}

#[derive(Debug)]
struct StackVec<T, const N: usize> {
    inner: MaybeUninit<[T; N]>,
    len: usize,
}

impl<T, const N: usize> Default for StackVec<T, N> {
    fn default() -> Self {
        Self {
            inner: MaybeUninit::uninit(),
            len: 0,
        }
    }
}

impl<T, const N: usize> StackVec<T, N> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, val: T) {
        let ptr = self.inner.as_mut_ptr() as *mut T;
        let ptr = unsafe { ptr.add(self.len) };
        unsafe { ptr.write(val) };
        self.len += 1;
    }

    pub fn pop(&mut self) {
        self.len -= 1;
    }

    pub fn last(&self) -> Option<&T> {
        self.as_slice().last()
    }

    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.as_mut_slice().last_mut()
    }

    pub fn as_slice(&self) -> &[T] {
        let ptr = self.inner.as_ptr() as *const T;
        unsafe { slice::from_raw_parts(ptr, self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let ptr = self.inner.as_ptr() as *mut T;
        unsafe { slice::from_raw_parts_mut(ptr, self.len) }
    }

    pub unsafe fn truncate(&mut self, len: usize) {
        if len > self.len {
            panic!()
        }
        self.len = len;
    }
}

/// Information about the machine we're running on, parsed from the Device Tree Blob (DTB) passed
/// to us by a previous boot stage (U-BOOT)
#[derive(Debug)]
pub struct MachineInfo {
    pub cpus: usize,
    pub serial: Serial,
    pub qemu_test: Option<Range<usize>>,
    pub memory: Range<usize>,
}

#[derive(Debug)]
pub struct Serial {
    pub reg: Range<usize>,
    pub clock_frequency: u32,
}

impl MachineInfo {
    pub fn from_dtb(dtb_ptr: *const u8) -> Self {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }.unwrap();

        let mut v = MachineInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        let info = MachineInfo {
            cpus: v.cpus,
            serial: v.serial.unwrap(),
            qemu_test: v.qemu_test,
            memory: Default::default(),
        };

        log::debug!("{info:#x?}");

        todo!()
    }
}

#[derive(Default, Debug)]
struct Reg {
    inner: Option<Range<usize>>,
    addr_size: usize,
    width_size: usize,
}

#[derive(Default)]
struct MachineInfoVisitor<'dt> {
    node: Option<Node<'dt>>,
    address_sizes: StackVec<usize, 16>,
    width_sizes: StackVec<usize, 16>,

    cpus: usize,
    serial: Option<Serial>,
    qemu_test: Option<Range<usize>>,
    memory: Option<Range<usize>>,
}

struct SerialVisitor {
    pub reg: RegVisitor,
    pub clock_frequency: Option<u32>,
}

impl SerialVisitor {
    fn new(addr_size: usize, width_size: usize) -> Self {
        Self {
            reg: RegVisitor::new(addr_size, width_size),
            clock_frequency: None,
        }
    }
}

struct RegVisitor {
    pub inner: Option<Range<usize>>,
    addr_size: usize,
    width_size: usize,
}

impl RegVisitor {
    fn new(addr_size: usize, width_size: usize) -> Self {
        Self {
            inner: None,
            addr_size,
            width_size,
        }
    }
}

impl<'dt> Visitor<'dt> for MachineInfoVisitor<'dt> {
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Error> {
        self.node = Some(node.clone());

        if name.starts_with("cpu@") {
            self.cpus += 1;
        } else if name.starts_with("memory@") {
            log::debug!(
                "{:?} {:?}",
                self.address_sizes.as_slice(),
                self.width_sizes.as_slice()
            );

            let mut v = RegVisitor::new(
                *self.address_sizes.last().unwrap(),
                *self.width_sizes.last().unwrap(),
            );
            node.visit(&mut v)?;
            self.memory = v.inner;
        } else {
            let addr_len = self.address_sizes.len;
            let width_len = self.width_sizes.len;

            node.visit(self)?;

            unsafe {
                self.address_sizes.truncate(addr_len);
                self.width_sizes.truncate(width_len)
            }
        }

        log::debug!(
            "{:?} {:?} {:?}",
            name,
            self.address_sizes.as_slice(),
            self.width_sizes.as_slice()
        );

        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.address_sizes.push(size_in_bytes);

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.width_sizes.push(size_in_bytes);

        Ok(())
    }

    fn visit_compatible(&mut self, mut strings: Strings<'dt>) -> Result<(), Error> {
        while let Some(str) = strings.next()? {
            match str {
                "sifive,test0" => {
                    if let Some(node) = self.node.take() {
                        let mut v = RegVisitor::new(
                            *self.address_sizes.last().unwrap(),
                            *self.width_sizes.last().unwrap(),
                        );
                        node.visit(&mut v)?;
                        self.qemu_test = v.inner;
                    }
                }
                "ns16550a" => {
                    if let Some(node) = self.node.take() {
                        let mut v = SerialVisitor::new(
                            *self.address_sizes.last().unwrap(),
                            *self.width_sizes.last().unwrap(),
                        );
                        node.visit(&mut v)?;

                        self.serial = Some(Serial {
                            reg: v.reg.inner.unwrap(),
                            clock_frequency: v.clock_frequency.unwrap(),
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl<'a> Visitor<'a> for SerialVisitor {
    fn visit_reg(&mut self, reg: &'a [u8]) -> Result<(), Error> {
        self.reg.visit_reg(reg)
    }

    fn visit_property(&mut self, name: &'a str, value: &'a [u8]) -> Result<(), Error> {
        if name == "clock-frequency" {
            self.clock_frequency = Some(u32::from_be_bytes(value.try_into().unwrap()));
        }

        Ok(())
    }
}

impl<'a> Visitor<'a> for RegVisitor {
    fn visit_reg(&mut self, reg: &[u8]) -> Result<(), Error> {
        assert_ne!(self.addr_size, 0);
        assert_ne!(self.width_size, 0);

        let (reg, rest) = reg.split_at(self.addr_size);
        let (width, _) = rest.split_at(self.width_size);

        let start = usize::from_be_bytes(reg.try_into().unwrap());
        let width = usize::from_be_bytes(width.try_into().unwrap());

        self.inner = Some(start..start + width);

        Ok(())
    }
}

// #[derive(Default, Debug)]
// struct MachineInfoVisitor {
//     pub cpus: usize,
//     pub serial_reg: Option<Range<usize>>,
//     pub serial_freq: Option<u32>,
//     pub qemu_test: Option<Range<usize>>,
//
//     reg: Reg,
// }
//
// impl<'dt> Visitor<'dt> for MachineInfoVisitor {
//     fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Error> {
//         if name.starts_with("cpu@") {
//             self.cpus += 1;
//         } else {
//             node.visit(self)?;
//         }
//
//         Ok(())
//     }
//
//     fn visit_address_cells(&mut self, addr_in_cells: u32) -> Result<(), Error> {
//         let size_in_bytes = addr_in_cells as usize * mem::size_of::<u32>();
//         self.reg.addr_size = size_in_bytes;
//         Ok(())
//     }
//
//     fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Error> {
//         let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();
//         self.reg.width_size = size_in_bytes;
//         Ok(())
//     }
//
//     fn visit_reg(&mut self, reg: &'dt [u8]) -> Result<(), Error> {
//         if self.reg.addr_size != mem::size_of::<usize>()
//             || self.reg.width_size != mem::size_of::<usize>()
//         {
//             log::debug!(
//                 "wrong cell sizes {} {}",
//                 self.reg.addr_size,
//                 self.reg.width_size
//             );
//             return Ok(());
//         }
//
//         let (reg, rest) = reg.split_at(self.reg.addr_size);
//         let (width, _) = rest.split_at(self.reg.width_size);
//
//         let reg = usize::from_be_bytes(reg.try_into().unwrap());
//         let width = usize::from_be_bytes(width.try_into().unwrap());
//
//         self.reg.inner = Some(reg..reg + width);
//
//         Ok(())
//     }
//
//     fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Error> {
//         match name {
//             "clock-frequency" => {
//                 self.serial_freq = Some(u32::from_be_bytes(value.try_into().unwrap()));
//             }
//             _ => {}
//         }
//         Ok(())
//     }
//
//     fn visit_compatible(&mut self, mut strings: Strings<'dt>) -> Result<(), Error> {
//         while let Some(str) = strings.next()? {
//             match str {
//                 "sifive,test0" => {
//                     self.qemu_test = Some(self.reg.inner.take().unwrap());
//                 }
//                 "ns16550a" => {
//                     self.serial_reg = Some(self.reg.inner.take().unwrap());
//                 }
//                 _ => {}
//             }
//         }
//         Ok(())
//     }
// }
