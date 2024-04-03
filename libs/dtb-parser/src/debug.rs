use crate::{Error, Node, Strings, Visitor};
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

impl<'a, 'dt, T: fmt::Write> Visitor<'dt> for DebugVisitor<'a, T> {
    type Error = Error;

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> crate::Result<()> {
        self.write(format_args!("- {}\n", name))?;

        self.padding += 4;
        node.visit(self)?;
        self.padding -= 4;

        Ok(())
    }
    fn visit_reg(&mut self, reg: &'dt [u8]) -> crate::Result<()> {
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

    fn visit_compatible(&mut self, mut strings: Strings<'dt>) -> Result<(), Error> {
        self.write(format_args!("compatible = "))?;

        while let Some(str) = strings.next()? {
            let _ = write!(self.write, "{str:?}, ");
        }

        self.write(format_args!("\n"))?;

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, bytes: &'dt [u8]) -> crate::Result<()> {
        self.write(format_args!("{:?} = {:?}\n", name, bytes))?;
        Ok(())
    }
}
