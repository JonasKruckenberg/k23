use crate::node::Node;
use crate::{c_strlen_on_slice, Error, Visit};
use core::{mem, str};

const FDT_BEGIN_NODE: u32 = 0x00000001;
const FDT_END_NODE: u32 = 0x00000002;
const FDT_PROP: u32 = 0x00000003;
const FDT_NOP: u32 = 0x00000004;
const FDT_END: u32 = 0x00000009;

macro_rules! align_down {
    ($value:expr, $alignment:expr) => {
        $value & !($alignment - 1)
    };
}

macro_rules! align_up {
    ($value:expr, $alignment:expr) => {
        align_down!($value + ($alignment - 1), $alignment)
    };
}

pub struct Parser<'a> {
    struct_slice: &'a [u8],
    strings_slice: &'a [u8],
    level: usize,
}

impl<'a> Parser<'a> {
    pub fn new(struct_slice: &'a [u8], strings_slice: &'a [u8], level: usize) -> Self {
        Self {
            struct_slice,
            strings_slice,
            level,
        }
    }

    pub fn walk(mut self, visitor: &mut dyn Visit<'a>) -> crate::Result<()> {
        let mut nesting_level = self.level;

        while !self.struct_slice.is_empty() {
            let token = self.read_u32()?;

            match token {
                FDT_BEGIN_NODE => {
                    nesting_level += 1;

                    // only parse the subnode if it's a direct child of the current node
                    if nesting_level == self.level + 1 {
                        let name = self.read_node_name()?;
                        let mut node =
                            Node::new(self.struct_slice, self.strings_slice, nesting_level);

                        // hack to skip over the root node
                        if name.is_empty() && self.level == 0 {
                            node.walk(visitor)?;
                        } else {
                            visitor.visit_subnode(name, node)?;
                        }
                    } else {
                        self.skip_node_name()?;
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

                    let aligned_len = align_up!(len, mem::size_of::<u32>());

                    let bytes = &self.struct_slice.get(..len).ok_or(Error::UnexpectedEOF)?;
                    self.struct_slice = &self
                        .struct_slice
                        .get(aligned_len..)
                        .ok_or(Error::UnexpectedEOF)?;

                    if nesting_level != self.level {
                        continue;
                    }

                    let name = self.read_prop_name(nameoff)?;

                    visitor.visit_property(name, bytes)?;
                }
                FDT_NOP => {}
                FDT_END => {
                    return if nesting_level != 0 || !self.struct_slice.is_empty() {
                        Err(Error::InvalidNesting)?
                    } else {
                        Ok(())
                    }
                }
                _ => return Err(Error::InvalidToken(token)),
            }
        }

        Ok(())
    }

    fn ensure_bytes(&self, len: usize) -> crate::Result<()> {
        if self.struct_slice.len() < len {
            Err(Error::UnexpectedEOF)?
        } else {
            Ok(())
        }
    }

    fn read_u32(&mut self) -> crate::Result<u32> {
        self.ensure_bytes(mem::size_of::<u32>())?;
        let (token_slice, rest) = self.struct_slice.split_at(mem::size_of::<u32>());
        self.struct_slice = rest;

        Ok(u32::from_be_bytes(token_slice.try_into()?))
    }

    fn read_node_name(&mut self) -> crate::Result<&'a str> {
        let node_name_len = c_strlen_on_slice(self.struct_slice);
        let aligned_len = align_up!(node_name_len + 1, mem::size_of::<u32>());

        let bytes = self
            .struct_slice
            .get(..node_name_len)
            .ok_or(Error::UnexpectedEOF)?;
        self.struct_slice = self
            .struct_slice
            .get(aligned_len..)
            .ok_or(Error::UnexpectedEOF)?;

        Ok(str::from_utf8(bytes)?)
    }

    fn skip_node_name(&mut self) -> crate::Result<()> {
        let node_name_len = c_strlen_on_slice(self.struct_slice);
        let aligned_len = align_up!(node_name_len + 1, mem::size_of::<u32>());

        self.struct_slice = self
            .struct_slice
            .get(aligned_len..)
            .ok_or(Error::UnexpectedEOF)?;

        Ok(())
    }

    fn read_prop_name(&self, nameoff: u32) -> crate::Result<&'a str> {
        let slice = &self
            .strings_slice
            .get(nameoff as usize..)
            .ok_or(Error::UnexpectedEOF)?;
        let prop_name_len = c_strlen_on_slice(slice);
        let prop_name = str::from_utf8(&slice[..prop_name_len])?;
        Ok(prop_name)
    }
}
