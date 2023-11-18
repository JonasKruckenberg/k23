use crate::parser::Parser;
use crate::Visit;

pub struct Node<'a> {
    parser: Parser<'a>,
}

impl<'a> Node<'a> {
    pub fn new(struct_slice: &'a [u8], strings_slice: &'a [u8], level: usize) -> Self {
        Self {
            parser: Parser::new(struct_slice, strings_slice, level),
        }
    }

    pub fn walk(self, visitor: &mut dyn Visit<'a>) -> crate::Result<()> {
        self.parser.walk(visitor)
    }
}
