use crate::arch;
use core::ffi::CStr;
use core::fmt::Write;
#[cfg(target_os = "none")]
use dtb_parser::{DevTree, Node, Visitor};
#[cfg(target_os = "none")]
pub use loader_api;

#[allow(unreachable_code)]
pub fn exit(code: i32) -> ! {
    #[cfg(target_os = "none")]
    arch::exit(code);

    #[cfg(not(target_os = "none"))]
    ::std::process::exit(code);
}

#[allow(unused)]
pub fn print(str: &str) {
    #[cfg(target_os = "none")]
    arch::hio::HostStream::new_stdout().write_str(str).unwrap();

    #[cfg(not(target_os = "none"))]
    use ::std::io::Write;
    #[cfg(not(target_os = "none"))]
    ::std::io::stdout().write(str.as_bytes()).unwrap();
}

#[cfg(target_os = "none")]
pub struct MachineInfo<'dt> {
    pub bootargs: Option<&'dt CStr>,
}

#[cfg(target_os = "none")]
impl<'dt> MachineInfo<'dt> {
    /// # Safety
    ///
    /// The caller has to ensure the provided pointer actually points to a FDT in memory.
    pub unsafe fn from_dtb(dtb_ptr: *const u8) -> Self {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }.unwrap();
        let mut v = BootInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        MachineInfo {
            bootargs: v.bootargs,
        }
    }
}

/*
----------------------------------------------------------------------------------------------------
    visitors
----------------------------------------------------------------------------------------------------
*/
#[cfg(target_os = "none")]
#[derive(Default)]
struct BootInfoVisitor<'dt> {
    bootargs: Option<&'dt CStr>,
}

#[cfg(target_os = "none")]
impl<'dt> Visitor<'dt> for BootInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name == "chosen" || name.is_empty() {
            node.visit(self)?;
        }

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "bootargs" {
            self.bootargs = Some(CStr::from_bytes_until_nul(value).unwrap());
        }

        Ok(())
    }
}
