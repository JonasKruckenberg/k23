#![allow(unused)]

macro_rules! nonzero {
    ($raw:literal) => {{ const { ::core::num::NonZero::new($raw).unwrap() } }};
}
pub(crate) use nonzero;
