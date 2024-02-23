use super::binary_reader::BinaryReader;
use super::instructions::Instruction;
use super::types::ValueType;
use core::fmt;
use core::fmt::Formatter;

pub struct FunctionBody<'a> {
    pub(crate) reader: BinaryReader<'a>,
}

#[derive(Clone)]
pub struct ConstExpr<'a> {
    pub(crate) reader: BinaryReader<'a>,
}

#[derive(Clone)]
pub struct InstructionsIter<'a> {
    reader: BinaryReader<'a>,
    done: bool,
    nesting: usize,
}

#[derive(Clone)]
pub struct Locals<'a> {
    reader: BinaryReader<'a>,
    total: u32,
    remaining: u32,
    err: bool,
}

impl<'a> ConstExpr<'a> {
    pub fn instructions(&self) -> InstructionsIter<'a> {
        InstructionsIter::new(self.reader.clone())
    }
}

impl<'a> fmt::Debug for ConstExpr<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ConstExpr")
            .field(&self.instructions())
            .finish()
    }
}

impl<'a> InstructionsIter<'a> {
    pub fn new(reader: BinaryReader<'a>) -> Self {
        Self {
            reader,
            done: false,
            nesting: 1,
        }
    }
}

impl<'a> Iterator for InstructionsIter<'a> {
    type Item = crate::Result<Instruction<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            None
        } else {
            let res = self.reader.read_instruction();
            self.done = res.is_err();

            if let Ok(instr) = &res {
                match instr {
                    Instruction::Block { .. }
                    | Instruction::If { .. }
                    | Instruction::Loop { .. }
                    | Instruction::Try { .. } => self.nesting += 1,
                    Instruction::End => {
                        self.nesting -= 1;
                        if self.nesting == 0 {
                            self.done = true;
                        }
                    }
                    _ => {}
                }
            }

            Some(res)
        }
    }
}

impl<'a> fmt::Debug for InstructionsIter<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

impl<'a> FunctionBody<'a> {
    pub fn len(&self) -> usize {
        self.reader.bytes.len()
    }

    pub fn locals(&self) -> crate::Result<Locals<'a>> {
        let mut reader = self.reader.clone();
        let count = reader.read_u32_leb128()?;

        Ok(Locals {
            reader,
            total: count,
            remaining: count,
            err: false,
        })
    }

    pub fn instructions(&self) -> crate::Result<InstructionsIter<'a>> {
        let mut reader = self.reader.clone();
        reader.skip_locals()?;
        Ok(InstructionsIter::new(reader))
    }
}

impl<'a> fmt::Debug for FunctionBody<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FunctionBody")
            .field(&self.instructions())
            .finish()
    }
}

impl<'a> Locals<'a> {
    pub fn len(&self) -> usize {
        self.total as usize
    }

    fn read(&mut self) -> crate::Result<(u32, ValueType)> {
        let count = self.reader.read_u32_leb128()?;
        let value_type = self.reader.read_value_type()?;
        Ok((count, value_type))
    }
}

impl<'a> Iterator for Locals<'a> {
    type Item = crate::Result<(u32, ValueType)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.err || self.remaining == 0 {
            return None;
        }

        let res = self.read();
        self.err = res.is_err();
        self.remaining -= 1;
        Some(res)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.total as usize, Some(self.total as usize))
    }
}

impl<'a> fmt::Debug for Locals<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut l = f.debug_list();
        let iter = self.clone();
        for entry in iter {
            let (count, ty) = entry.unwrap();
            for _ in 0..count {
                l.entry(&ty);
            }
        }

        l.finish()
    }
}
