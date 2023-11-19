use crate::{Node, Visit};
use core::fmt;

pub struct DebugVisitor<'a, T: fmt::Write> {
    write: &'a mut T,
    padding: usize,
}

impl<'a, T: fmt::Write> DebugVisitor<'a, T> {
    pub fn new(write: &'a mut T) -> Self {
        Self { write, padding: 0 }
    }

    fn write(&mut self, args: fmt::Arguments<'_>) -> crate::Result<()> {
        let _ = write!(self.write, "{:indent$}{}", "", args, indent = self.padding);
        Ok(())
    }
}

impl<'a, 'b, T: fmt::Write> Visit<'b> for DebugVisitor<'a, T> {
    fn visit_subnode(&mut self, name: &'b str, mut node: Node<'b>) -> crate::Result<()> {
        self.write(format_args!("- {}\n", name))?;

        self.padding += 4;
        node.walk(self)?;
        self.padding -= 4;

        Ok(())
    }
    fn visit_reg(&mut self, reg: &'b [u8]) -> crate::Result<()> {
        self.write(format_args!("reg = {:?}\n", reg))?;
        Ok(())
    }
    fn visit_address_cells(&mut self, cells: u32) -> crate::Result<()> {
        self.write(format_args!("#address-cells = {}\n", cells))?;

        Ok(())
    }

    fn visit_size_cells(&mut self, cells: u32) -> crate::Result<()> {
        self.write(format_args!("#size-cells = {}\n", cells))?;

        Ok(())
    }

    fn visit_property(&mut self, name: &'b str, bytes: &'b [u8]) -> crate::Result<()> {
        self.write(format_args!("{:?} = {:?}\n", name, bytes))?;
        Ok(())
    }
}
