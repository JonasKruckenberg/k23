use crate::core::{HeapType, V128Const};
use crate::kw;
use crate::parser::{Cursor, Parse, Parser, Peek, Result};
use crate::token::{F32, F64, Index};
use alloc::vec::Vec;

/// Expression that can be used inside of `invoke` expressions for core wasm
/// functions.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum WastArgCore<'a> {
    I32(i32),
    I64(i64),
    F32(F32),
    F64(F64),
    V128(V128Const),
    RefNull(HeapType<'a>),
    RefExtern(u32),
    RefHost(u32),
}

#[expect(clippy::type_complexity, reason = "")]
static ARGS: &[(&str, fn(Parser<'_>) -> Result<WastArgCore<'_>>)] = {
    use WastArgCore::*;
    &[
        ("i32.const", |p| Ok(I32(p.parse()?))),
        ("i64.const", |p| Ok(I64(p.parse()?))),
        ("f32.const", |p| Ok(F32(p.parse()?))),
        ("f64.const", |p| Ok(F64(p.parse()?))),
        ("v128.const", |p| Ok(V128(p.parse()?))),
        ("ref.null", |p| Ok(RefNull(p.parse()?))),
        ("ref.extern", |p| Ok(RefExtern(p.parse()?))),
        ("ref.host", |p| Ok(RefHost(p.parse()?))),
    ]
};

impl<'a> Parse<'a> for WastArgCore<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let parse = parser.step(|c| {
            if let Some((kw, rest)) = c.keyword()?
                && let Some(i) = ARGS.iter().position(|(name, _)| *name == kw)
            {
                return Ok((ARGS[i].1, rest));
            }
            Err(c.error("expected a [type].const expression"))
        })?;
        parse(parser)
    }
}

impl Peek for WastArgCore<'_> {
    fn peek(cursor: Cursor<'_>) -> Result<bool> {
        let kw = match cursor.keyword()? {
            Some((kw, _)) => kw,
            None => return Ok(false),
        };
        Ok(ARGS.iter().any(|(name, _)| *name == kw))
    }

    fn display() -> &'static str {
        "core wasm argument"
    }
}

/// Expressions that can be used inside of `assert_return` to validate the
/// return value of a core wasm function.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum WastRetCore<'a> {
    I32(i32),
    I64(i64),
    F32(NanPattern<F32>),
    F64(NanPattern<F64>),
    V128(V128Pattern),

    /// A null reference is expected, optionally with a specified type.
    RefNull(Option<HeapType<'a>>),
    /// A non-null externref is expected which should contain the specified
    /// value.
    RefExtern(Option<u32>),
    /// A non-null anyref is expected which should contain the specified host value.
    RefHost(u32),
    /// A non-null funcref is expected.
    RefFunc(Option<Index<'a>>),
    /// A non-null anyref is expected.
    RefAny,
    /// A non-null eqref is expected.
    RefEq,
    /// A non-null arrayref is expected.
    RefArray,
    /// A non-null structref is expected.
    RefStruct,
    /// A non-null i31ref is expected.
    RefI31,
    /// A non-null, shared i31ref is expected.
    RefI31Shared,

    Either(Vec<WastRetCore<'a>>),
}

#[expect(clippy::type_complexity, reason = "")]
static RETS: &[(&str, fn(Parser<'_>) -> Result<WastRetCore<'_>>)] = {
    use WastRetCore::*;
    &[
        ("i32.const", |p| Ok(I32(p.parse()?))),
        ("i64.const", |p| Ok(I64(p.parse()?))),
        ("f32.const", |p| Ok(F32(p.parse()?))),
        ("f64.const", |p| Ok(F64(p.parse()?))),
        ("v128.const", |p| Ok(V128(p.parse()?))),
        ("ref.null", |p| Ok(RefNull(p.parse()?))),
        ("ref.extern", |p| Ok(RefExtern(p.parse()?))),
        ("ref.host", |p| Ok(RefHost(p.parse()?))),
        ("ref.func", |p| Ok(RefFunc(p.parse()?))),
        ("ref.any", |_| Ok(RefAny)),
        ("ref.eq", |_| Ok(RefEq)),
        ("ref.array", |_| Ok(RefArray)),
        ("ref.struct", |_| Ok(RefStruct)),
        ("ref.i31", |_| Ok(RefI31)),
        ("ref.i31_shared", |_| Ok(RefI31Shared)),
        ("either", |p| {
            p.depth_check()?;
            let mut cases = Vec::new();
            while !p.is_empty() {
                cases.push(p.parens(|p| p.parse())?);
            }
            Ok(Either(cases))
        }),
    ]
};

impl<'a> Parse<'a> for WastRetCore<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let parse = parser.step(|c| {
            if let Some((kw, rest)) = c.keyword()?
                && let Some(i) = RETS.iter().position(|(name, _)| *name == kw)
            {
                return Ok((RETS[i].1, rest));
            }
            Err(c.error("expected a [type].const expression"))
        })?;
        parse(parser)
    }
}

impl Peek for WastRetCore<'_> {
    fn peek(cursor: Cursor<'_>) -> Result<bool> {
        let kw = match cursor.keyword()? {
            Some((kw, _)) => kw,
            None => return Ok(false),
        };
        Ok(RETS.iter().any(|(name, _)| *name == kw))
    }

    fn display() -> &'static str {
        "core wasm return value"
    }
}

/// Either a NaN pattern (`nan:canonical`, `nan:arithmetic`) or a value of type `T`.
#[derive(Copy, Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum NanPattern<T> {
    CanonicalNan,
    ArithmeticNan,
    Value(T),
}

impl<'a, T> Parse<'a> for NanPattern<T>
where
    T: Parse<'a>,
{
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if parser.peek::<kw::nan_canonical>()? {
            parser.parse::<kw::nan_canonical>()?;
            Ok(NanPattern::CanonicalNan)
        } else if parser.peek::<kw::nan_arithmetic>()? {
            parser.parse::<kw::nan_arithmetic>()?;
            Ok(NanPattern::ArithmeticNan)
        } else {
            let val = parser.parse()?;
            Ok(NanPattern::Value(val))
        }
    }
}

/// A version of `V128Const` that allows `NanPattern`s.
///
/// This implementation is necessary because only float types can include NaN patterns; otherwise
/// it is largely similar to the implementation of `V128Const`.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub enum V128Pattern {
    I8x16([i8; 16]),
    I16x8([i16; 8]),
    I32x4([i32; 4]),
    I64x2([i64; 2]),
    F32x4([NanPattern<F32>; 4]),
    F64x2([NanPattern<F64>; 2]),
}

impl<'a> Parse<'a> for V128Pattern {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut l = parser.lookahead1();
        if l.peek::<kw::i8x16>()? {
            parser.parse::<kw::i8x16>()?;
            Ok(V128Pattern::I8x16([
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
            ]))
        } else if l.peek::<kw::i16x8>()? {
            parser.parse::<kw::i16x8>()?;
            Ok(V128Pattern::I16x8([
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
            ]))
        } else if l.peek::<kw::i32x4>()? {
            parser.parse::<kw::i32x4>()?;
            Ok(V128Pattern::I32x4([
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
            ]))
        } else if l.peek::<kw::i64x2>()? {
            parser.parse::<kw::i64x2>()?;
            Ok(V128Pattern::I64x2([parser.parse()?, parser.parse()?]))
        } else if l.peek::<kw::f32x4>()? {
            parser.parse::<kw::f32x4>()?;
            Ok(V128Pattern::F32x4([
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
                parser.parse()?,
            ]))
        } else if l.peek::<kw::f64x2>()? {
            parser.parse::<kw::f64x2>()?;
            Ok(V128Pattern::F64x2([parser.parse()?, parser.parse()?]))
        } else {
            Err(l.error())
        }
    }
}
