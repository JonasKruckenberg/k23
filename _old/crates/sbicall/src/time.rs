use crate::{sbi_call, EID_TIME};

#[inline]
pub fn set_timer(stime_value: u64) -> super::Result<()> {
    #[cfg(target_pointer_width = "64")]
    sbi_call!(ext: EID_TIME, func: 0, "a0": usize::try_from(stime_value)?, "a1": 0)?;
    #[cfg(target_pointer_width = "32")]
    sbi_call!(
        ext: EID_TIME,
        func: 0,
        "a0": stime_value as usize,
        "a1": (stime_value >> 32) as usize,
    )?;

    Ok(())
}
