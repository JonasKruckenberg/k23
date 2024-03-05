use crate::{sbi_call, EID_TIME};

#[inline]
pub fn set_timer(stime_value: u64) -> super::Result<()> {
    sbi_call!(ext: EID_TIME, func: 0, "a0": usize::try_from(stime_value)?, "a1": 0)?;

    Ok(())
}
