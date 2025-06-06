use crate::component::Component;
use crate::core::{Module, ModuleField, ModuleKind};
use crate::kw;
use crate::parser::{Parse, Parser, Result};
use crate::token::Span;
use alloc::vec::Vec;

/// A `*.wat` file parser, or a parser for one parenthesized module.
///
/// This is the top-level type which you'll frequently parse when working with
/// this crate. A `*.wat` file is either one `module` s-expression or a sequence
/// of s-expressions that are module fields.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum Wat<'a> {
    Module(Module<'a>),
    Component(Component<'a>),
}

impl Wat<'_> {
    fn validate(&self, parser: Parser<'_>) -> Result<()> {
        match self {
            Wat::Module(m) => m.validate(parser),
            Wat::Component(c) => c.validate(parser),
        }
    }

    /// Encodes this `Wat` to binary form. This calls either [`Module::encode`]
    /// or [`Component::encode`].
    pub fn encode(&mut self) -> core::result::Result<Vec<u8>, crate::Error> {
        crate::core::EncodeOptions::default().encode_wat(self)
    }

    /// Returns the defining span of this file.
    pub fn span(&self) -> Span {
        match self {
            Wat::Module(m) => m.span,
            Wat::Component(c) => c.span,
        }
    }
}

impl<'a> Parse<'a> for Wat<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if !parser.has_meaningful_tokens() {
            return Err(parser.error("expected at least one module field"));
        }

        parser.with_standard_annotations_registered(|parser| {
            let wat = if parser.peek2::<kw::module>()? {
                Wat::Module(parser.parens(|parser| parser.parse())?)
            } else if parser.peek2::<kw::component>()? {
                Wat::Component(parser.parens(|parser| parser.parse())?)
            } else {
                let fields = ModuleField::parse_remaining(parser)?;
                Wat::Module(Module {
                    span: Span { offset: 0 },
                    id: None,
                    name: None,
                    kind: ModuleKind::Text(fields),
                })
            };
            wat.validate(parser)?;
            Ok(wat)
        })
    }
}
