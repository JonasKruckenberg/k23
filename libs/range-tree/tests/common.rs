macro_rules! idx {
    ($nonmax:ident($raw:literal)) => {{ const { $nonmax::new($raw).unwrap() } }};
}
pub(crate) use idx;
