use crate::{
    read_str, Error, Node, Strings, Visitor, FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_NOP,
    FDT_PROP,
};
use core::mem;

#[derive(Debug, Clone)]
pub struct Parser<'dt> {
    pub struct_slice: &'dt [u8],
    pub strings_slice: &'dt [u8],
    /// Offset into the `struct_slice` buffer
    pub offset: usize,
    pub level: usize,
}

impl<'dt> Parser<'dt> {
    pub fn visit<E: core::error::Error + From<Error>>(
        &mut self,
        visitor: &mut dyn Visitor<'dt, Error = E>,
    ) -> Result<(), E> {
        let mut nesting_level = self.level;

        while self.offset < self.struct_slice.len() {
            let token = self.read_u32()?;

            match token {
                FDT_BEGIN_NODE => {
                    nesting_level += 1;

                    let name = read_str(
                        self.struct_slice,
                        u32::try_from(self.offset).map_err(Error::from)?,
                    )?;
                    self.offset += align_up(name.len() + 1, mem::size_of::<u32>());

                    if nesting_level == self.level + 1 {
                        let node = Node::new(
                            self.struct_slice,
                            self.strings_slice,
                            self.offset,
                            nesting_level,
                        );

                        // hack to skip over the root node
                        // if name.is_empty() && self.level == 0 {
                        //     node.visit(visitor)?;
                        // } else {
                        visitor.visit_subnode(name, node)?;
                        // }
                    }
                }
                FDT_END_NODE => {
                    if nesting_level <= self.level {
                        return Ok(());
                    }

                    nesting_level -= 1;
                }
                FDT_PROP => {
                    let len = self.read_u32()? as usize;
                    let nameoff = self.read_u32()?;

                    let aligned_len = align_up(len, mem::size_of::<u32>());

                    let bytes = self.read_bytes(len)?;
                    self.offset += aligned_len - len;

                    if nesting_level != self.level {
                        continue;
                    }

                    let name = read_str(self.strings_slice, nameoff)?;

                    match name {
                        "reg" => visitor.visit_reg(bytes)?,
                        "#address-cells" => {
                            visitor.visit_address_cells(u32::from_be_bytes(
                                bytes.try_into().map_err(Into::into)?,
                            ))?;
                        }
                        "#size-cells" => {
                            visitor.visit_size_cells(u32::from_be_bytes(
                                bytes.try_into().map_err(Into::into)?,
                            ))?;
                        }
                        "compatible" => {
                            visitor.visit_compatible(Strings::new(bytes))?;
                        }
                        _ => visitor.visit_property(name, bytes)?,
                    }
                }
                FDT_NOP => {}
                FDT_END => {
                    return if nesting_level != 0 || self.offset != self.struct_slice.len() {
                        Err(Error::InvalidNesting)?
                    } else {
                        Ok(())
                    };
                }
                _ => return Err(Error::InvalidToken(token).into()),
            }
        }

        Ok(())
    }

    fn read_bytes(&mut self, n: usize) -> crate::Result<&'dt [u8]> {
        let slice = self
            .struct_slice
            .get(self.offset..self.offset + n)
            .ok_or(Error::UnexpectedEOF)?;
        self.offset += n;

        Ok(slice)
    }

    fn read_u32(&mut self) -> crate::Result<u32> {
        let bytes = self.read_bytes(mem::size_of::<u32>())?;

        Ok(u32::from_be_bytes(bytes.try_into()?))
    }
}

fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

fn align_up(value: usize, alignment: usize) -> usize {
    align_down(value + (alignment - 1), alignment)
}
