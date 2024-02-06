macro_rules! impl_read_unsigned_leb128 {
    ($fn_name:ident, $int_ty:ty) => {
        #[inline]
        pub fn $fn_name(&mut self) -> $crate::Result<$int_ty> {
            // The first iteration of this loop is unpeeled. This is a
            // performance win because this code is hot and integer values less
            // than 128 are very common, typically occurring 50-80% or more of
            // the time, even for u64 and u128.
            let byte = self.read_u8()?;
            if (byte & 0x80) == 0 {
                return Ok(byte as $int_ty);
            }
            let mut result = (byte & 0x7F) as $int_ty;
            let mut shift = 7;
            loop {
                let byte = self.read_u8()?;
                if (byte & 0x80) == 0 {
                    result |= (byte as $int_ty) << shift;
                    return Ok(result);
                } else {
                    result |= ((byte & 0x7F) as $int_ty) << shift;
                }
                shift += 7;
            }
        }
    };
}

macro_rules! impl_read_signed_leb128 {
    ($fn_name:ident, $int_ty:ty) => {
        #[inline]
        pub fn $fn_name(&mut self) -> $crate::Result<$int_ty> {
            let mut result = 0;
            let mut shift = 0;
            let mut byte;

            loop {
                byte = self.read_u8()?;
                result |= <$int_ty>::from(byte & 0x7F) << shift;
                shift += 7;

                if (byte & 0x80) == 0 {
                    break;
                }
            }

            if (shift < <$int_ty>::BITS) && ((byte & 0x40) != 0) {
                // sign extend
                result |= (!0 << shift);
            }

            Ok(result)
        }
    };
}

pub(crate) use impl_read_signed_leb128;
pub(crate) use impl_read_unsigned_leb128;
