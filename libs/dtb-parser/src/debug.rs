use crate::{Error, Node, Strings, Visitor};
use core::fmt;
use fallible_iterator::FallibleIterator;

#[allow(clippy::module_name_repetitions)]
pub struct DebugVisitor<'a, T: fmt::Write> {
    write: &'a mut T,
    padding: usize,
}

impl<'a, T: fmt::Write> DebugVisitor<'a, T> {
    pub fn new(write: &'a mut T) -> Self {
        Self { write, padding: 0 }
    }

    fn write(&mut self, args: fmt::Arguments<'_>) {
        let _ = write!(self.write, "{:indent$}{}", "", args, indent = self.padding);
    }
}

impl<'dt, T: fmt::Write> Visitor<'dt> for DebugVisitor<'_, T> {
    type Error = Error;

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> crate::Result<()> {
        self.write(format_args!("- {name}\n"));

        self.padding += 4;
        node.visit(self)?;
        self.padding -= 4;

        Ok(())
    }
    fn visit_reg(&mut self, reg: &'dt [u8]) -> crate::Result<()> {
        self.write(format_args!("reg = {reg:?}\n"));
        Ok(())
    }
    fn visit_address_cells(&mut self, cells: u32) -> crate::Result<()> {
        self.write(format_args!("#address-cells = {cells}\n"));

        Ok(())
    }

    fn visit_size_cells(&mut self, cells: u32) -> crate::Result<()> {
        self.write(format_args!("#size-cells = {cells}\n"));

        Ok(())
    }

    fn visit_compatible(&mut self, mut strings: Strings<'dt>) -> Result<(), Error> {
        self.write(format_args!("compatible = "));

        while let Some(str) = strings.next()? {
            let _ = write!(self.write, "{str:?}, ");
        }

        self.write(format_args!("\n"));

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, bytes: &'dt [u8]) -> crate::Result<()> {
        self.write(format_args!("{name:?} = {bytes:?}\n"));
        Ok(())
    }
}
