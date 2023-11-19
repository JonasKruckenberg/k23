use super::{sbi_call, EID_TIME};

#[inline]
pub fn set_timer(stime_value: u64) -> super::Result<()> {
    #[cfg(target_pointer_width = "64")]
    sbi_call(EID_TIME, 0, stime_value as usize, 0, 0)?;
    #[cfg(target_pointer_width = "32")]
    sbi_call(
        EID_TIME,
        0,
        stime_value as usize,
        (stime_value >> 32) as usize,
        0,
    )?;

    Ok(())
}
