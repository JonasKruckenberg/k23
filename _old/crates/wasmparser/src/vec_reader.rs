use crate::binary_reader::BinaryReader;
use core::fmt;
use core::fmt::Formatter;

pub struct VecReader<'a, T> {
    reader: BinaryReader<'a>,
    len: u32,
    limit: Option<usize>,
    ctor: fn(&mut BinaryReader<'a>) -> crate::Result<T>,
}

impl<'a, T> Clone for VecReader<'a, T> {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
            len: self.len,
            limit: self.limit,
            ctor: self.ctor,
        }
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for VecReader<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a, T> VecReader<'a, T> {
    pub fn new(
        bytes: &'a [u8],
        ctor: fn(&mut BinaryReader<'a>) -> crate::Result<T>,
        limit: Option<usize>,
    ) -> crate::Result<Self> {
        let mut reader = BinaryReader::new(bytes);
        let len = reader.read_u32_leb128()?;

        Ok(Self {
            reader,
            len,
            ctor,
            limit,
        })
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> VecIter<'a, T> {
        VecIter {
            reader: self.clone(),
            remaining: self.len,
            done: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VecIter<'a, T> {
    reader: VecReader<'a, T>,
    remaining: u32,
    done: bool,
}

impl<'a, T> Iterator for VecIter<'a, T> {
    type Item = crate::Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        if self.remaining == 0 {
            self.done = true;
            None
        } else {
            let res = (self.reader.ctor)(&mut self.reader.reader);
            self.done = res.is_err();
            self.remaining -= 1;
            Some(res)
        }
    }
}
