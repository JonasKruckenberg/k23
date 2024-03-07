use crate::arch;
use arrayvec::ArrayVec;
use core::mem;
use core::ops::Range;
use core::ptr::NonNull;
use dtb_parser::{DevTree, Node, Strings, Visitor};
use spin::Once;

pub static BOOT_INFO: Once<BootInfo> = Once::new();

/// Information about the machine we're running on, parsed from the Device Tree Blob (DTB) passed
/// to us by a previous boot stage (U-BOOT)
pub struct BootInfo {
    /// The number of "standalone" CPUs in the system
    pub cpus: usize,
    /// Information about the systems UART device
    pub serial: Serial,
    /// Information about the systems QEMU test device, if present.
    ///
    /// This is currently only implemented for sifive (i.e. riscv) compatible devices and can be used
    /// to exit the hosting virtual machine on panics or after tests finished.
    pub qemu_exit_handle: Option<arch::QEMUExit>,
    /// The address range at which the primary physical memory of the system is mapped.
    pub memory: Range<usize>,
}

/// Information about the systems UART device
#[derive(Debug)]
pub struct Serial {
    /// The MMIO registers reserved for this device
    pub reg: Range<usize>,
    /// The clock frequency configured
    pub clock_frequency: u32,
}

impl BootInfo {
    pub fn from_dtb(dtb_ptr: NonNull<u8>) -> Self {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }.unwrap();

        let mut v = BootInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        v.result()
            .expect("failed to parse required info from device tree")
    }
}

/*
----------------------------------------------------------------------------------------------------
    FDT visitors
----------------------------------------------------------------------------------------------------
*/

#[derive(Default)]
struct BootInfoVisitor<'dt> {
    /// The most recent node we encountered.
    ///
    /// Since we decide to continue parsing a node depending on its `compatible` prop
    /// and props come *after* their respective node, we need to backtrack and therefore store the node.
    node: Option<Node<'dt>>,
    /// Stack of encountered `#address-cells` values, used to correctly read `reg` properties.
    ///
    /// The fixed upper bound of 8 elements is just a guess, technically FDTs can have arbitrary depth.
    /// But we don't have an allocator and 8 seems to be a reasonable choice in practice.
    address_sizes: ArrayVec<usize, 8>,
    /// Stack of encountered `#size-cells` values, used to correctly read `reg` properties.
    width_sizes: ArrayVec<usize, 8>,

    // values parsed from the FDT that we need to construct a `BootInfo` instance
    cpus: usize,
    serial: Option<Serial>,
    memory: Option<Range<usize>>,
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    sifive_test: Option<Range<usize>>,
}

struct SerialVisitor {
    pub reg: RegVisitor,
    pub clock_frequency: Option<u32>,
}

#[derive(Default)]
struct RegVisitor {
    pub inner: Option<Range<usize>>,
    address_size: usize,
    width_size: usize,
}

impl<'dt> BootInfoVisitor<'dt> {
    pub fn result(self) -> Option<BootInfo> {
        debug_assert_ne!(self.cpus, 0);

        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                let qemu_exit_handle = self
                    .sifive_test
                    .map(|reg| qemu_exit::RISCV64::new(reg.start as u64));
            } else if #[cfg(target_arch = "aarch64")]{
                let qemu_exit_handle = Some(qemu_exit::AArch64::new());
            } else if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
                let qemu_exit_handle = Some(qemu_exit::X86::new(0xf4, 0x10));
            } else {
                let qemu_exit_handle = None;
            }
        }

        Some(BootInfo {
            cpus: self.cpus,
            serial: self.serial?,
            qemu_exit_handle,
            memory: self.memory?,
        })
    }

    fn reg_visitor(&self) -> RegVisitor {
        RegVisitor {
            inner: None,
            address_size: *self.address_sizes.last().unwrap(),
            width_size: *self.width_sizes.last().unwrap(),
        }
    }
}

impl<'dt> Visitor<'dt> for BootInfoVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        self.node = Some(node.clone());

        if name.starts_with("cpu@") {
            self.cpus += 1;
        } else if name.starts_with("memory@") {
            let mut v = self.reg_visitor();
            node.visit(&mut v)?;
            self.memory = v.result();
        } else {
            let addr_len = self.address_sizes.len();
            let width_len = self.width_sizes.len();

            node.visit(self)?;

            self.address_sizes.truncate(addr_len);
            self.width_sizes.truncate(width_len);
        }

        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.address_sizes.push(size_in_bytes);

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.width_sizes.push(size_in_bytes);

        Ok(())
    }

    fn visit_compatible(&mut self, mut strings: Strings<'dt>) -> Result<(), Self::Error> {
        while let Some(str) = strings.next()? {
            match str {
                #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
                "sifive,test0" => {
                    if let Some(node) = self.node.take() {
                        let mut v = self.reg_visitor();
                        node.visit(&mut v)?;
                        self.sifive_test = v.result();
                    }
                }
                "ns16550a" => {
                    if let Some(node) = self.node.take() {
                        let mut v = SerialVisitor {
                            reg: self.reg_visitor(),
                            clock_frequency: None,
                        };
                        node.visit(&mut v)?;
                        self.serial = v.result();
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl SerialVisitor {
    pub fn result(self) -> Option<Serial> {
        Some(Serial {
            reg: self.reg.result()?,
            clock_frequency: self.clock_frequency?,
        })
    }
}

impl<'dt> Visitor<'dt> for SerialVisitor {
    type Error = dtb_parser::Error;

    fn visit_reg(&mut self, reg: &'dt [u8]) -> Result<(), Self::Error> {
        self.reg.visit_reg(reg)
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "clock-frequency" {
            self.clock_frequency = Some(u32::from_be_bytes(value.try_into().unwrap()));
        }

        Ok(())
    }
}

impl RegVisitor {
    pub fn result(self) -> Option<Range<usize>> {
        self.inner
    }
}

impl<'dt> Visitor<'dt> for RegVisitor {
    type Error = dtb_parser::Error;

    fn visit_reg(&mut self, reg: &[u8]) -> Result<(), Self::Error> {
        debug_assert_ne!(self.address_size, 0);
        debug_assert_ne!(self.width_size, 0);

        let (reg, rest) = reg.split_at(self.address_size);
        let (width, _) = rest.split_at(self.width_size);

        let start = usize::from_be_bytes(reg.try_into().unwrap());
        let width = usize::from_be_bytes(width.try_into().unwrap());

        self.inner = Some(start..start + width);

        Ok(())
    }
}
