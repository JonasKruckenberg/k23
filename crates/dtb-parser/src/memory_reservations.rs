use core::mem;

#[derive(Debug)]
pub struct MemoryReservation {
    /// Starting address of the reserved memory region
    pub address: u64,
    /// Size of the reserved memory region
    pub size: u64,
}

pub struct MemoryReservations<'a> {
    pub(crate) buf: &'a [u8],
    pub(crate) done: bool,
}

impl<'a> MemoryReservations<'a> {
    fn read_u64(&mut self) -> crate::Result<u64> {
        let (buf, rest) = self.buf.split_at(mem::size_of::<u64>());
        self.buf = rest;

        Ok(u64::from_be_bytes(buf.try_into()?))
    }

    fn read(&mut self) -> crate::Result<MemoryReservation> {
        let address = self.read_u64()?;
        let size = self.read_u64()?;

        Ok(MemoryReservation { address, size })
    }
}

impl<'a> Iterator for MemoryReservations<'a> {
    type Item = crate::Result<MemoryReservation>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buf.is_empty() || self.done {
            None
        } else {
            let entry = self.read();
            self.done = entry.is_err()
                || entry
                    .as_ref()
                    .map(|e| e.address == 0 || e.size == 0)
                    .unwrap_or_default();

            Some(entry)
        }
    }
}
