/// Errors returned by SBI calls
#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("Failed")]
    InvalidFieldValue {
        field: &'static str,
        value: usize,
        bitmask: usize,
    },
}
