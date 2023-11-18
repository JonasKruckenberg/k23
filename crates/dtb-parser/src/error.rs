#[derive(Debug, onlyerror::Error)]
pub enum Error {
    InvalidMagic,
    InvalidVersion,
}
