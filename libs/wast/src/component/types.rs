use crate::component::*;
use crate::core;
use crate::kw;
use crate::parser::Lookahead1;
use crate::parser::Peek;
use crate::parser::{Parse, Parser, Result};
use crate::token::Index;
use crate::token::LParen;
use crate::token::{Id, NameAnnotation, Span};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// A core type declaration.
#[derive(Debug)]
pub struct CoreType<'a> {
    /// Where this type was defined.
    pub span: Span,
    /// An optional identifier to refer to this `core type` by as part of name
    /// resolution.
    pub id: Option<Id<'a>>,
    /// An optional name for this type stored in the custom `name` section.
    pub name: Option<NameAnnotation<'a>>,
    /// The core type's definition.
    pub def: CoreTypeDef<'a>,
}

impl<'a> Parse<'a> for CoreType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let span = parser.parse::<kw::core>()?.0;
        parser.parse::<kw::r#type>()?;
        let id = parser.parse()?;
        let name = parser.parse()?;
        let def = parser.parens(|p| p.parse())?;

        Ok(Self {
            span,
            id,
            name,
            def,
        })
    }
}

/// Represents a core type definition.
///
/// In the future this may be removed when module types are a part of
/// a core module.
#[derive(Debug)]
pub enum CoreTypeDef<'a> {
    /// The type definition is one of the core types.
    Def(core::TypeDef<'a>),
    /// The type definition is a module type.
    Module(ModuleType<'a>),
}

impl<'a> Parse<'a> for CoreTypeDef<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if parser.peek::<kw::module>()? {
            parser.parse::<kw::module>()?;
            Ok(Self::Module(parser.parse()?))
        } else {
            Ok(Self::Def(parser.parse()?))
        }
    }
}

/// A type definition for a core module.
#[derive(Debug)]
pub struct ModuleType<'a> {
    /// The declarations of the module type.
    pub decls: Vec<ModuleTypeDecl<'a>>,
}

impl<'a> Parse<'a> for ModuleType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.depth_check()?;
        Ok(Self {
            decls: parser.parse()?,
        })
    }
}

/// The declarations of a [`ModuleType`].
#[derive(Debug)]
pub enum ModuleTypeDecl<'a> {
    /// A core type.
    Type(core::Type<'a>),
    /// A core recursion group.
    Rec(core::Rec<'a>),
    /// An alias local to the component type.
    Alias(Alias<'a>),
    /// An import.
    Import(core::Import<'a>),
    /// An export.
    Export(&'a str, core::ItemSig<'a>),
}

impl<'a> Parse<'a> for ModuleTypeDecl<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut l = parser.lookahead1();
        if l.peek::<kw::r#type>()? {
            Ok(Self::Type(parser.parse()?))
        } else if l.peek::<kw::rec>()? {
            Ok(Self::Rec(parser.parse()?))
        } else if l.peek::<kw::alias>()? {
            Ok(Self::Alias(Alias::parse_outer_core_type_alias(parser)?))
        } else if l.peek::<kw::import>()? {
            Ok(Self::Import(parser.parse()?))
        } else if l.peek::<kw::export>()? {
            parser.parse::<kw::export>()?;
            let name = parser.parse()?;
            let et = parser.parens(|parser| parser.parse())?;
            Ok(Self::Export(name, et))
        } else {
            Err(l.error())
        }
    }
}

impl<'a> Parse<'a> for Vec<ModuleTypeDecl<'a>> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut decls = Vec::new();
        while !parser.is_empty() {
            decls.push(parser.parens(|parser| parser.parse())?);
        }
        Ok(decls)
    }
}

/// A type declaration in a component.
#[derive(Debug)]
pub struct Type<'a> {
    /// Where this type was defined.
    pub span: Span,
    /// An optional identifier to refer to this `type` by as part of name
    /// resolution.
    pub id: Option<Id<'a>>,
    /// An optional name for this type stored in the custom `name` section.
    pub name: Option<NameAnnotation<'a>>,
    /// If present, inline export annotations which indicate names this
    /// definition should be exported under.
    pub exports: InlineExport<'a>,
    /// The type definition.
    pub def: TypeDef<'a>,
}

impl<'a> Type<'a> {
    /// Parses a `Type` while allowing inline `(export "...")` names to be
    /// defined.
    pub fn parse_maybe_with_inline_exports(parser: Parser<'a>) -> Result<Self> {
        Type::parse(parser, true)
    }

    fn parse_no_inline_exports(parser: Parser<'a>) -> Result<Self> {
        Type::parse(parser, false)
    }

    fn parse(parser: Parser<'a>, allow_inline_exports: bool) -> Result<Self> {
        let span = parser.parse::<kw::r#type>()?.0;
        let id = parser.parse()?;
        let name = parser.parse()?;
        let exports = if allow_inline_exports {
            parser.parse()?
        } else {
            Default::default()
        };
        let def = parser.parse()?;

        Ok(Self {
            span,
            id,
            name,
            exports,
            def,
        })
    }
}

/// A definition of a component type.
#[derive(Debug)]
pub enum TypeDef<'a> {
    /// A defined value type.
    Defined(ComponentDefinedType<'a>),
    /// A component function type.
    Func(ComponentFunctionType<'a>),
    /// A component type.
    Component(ComponentType<'a>),
    /// An instance type.
    Instance(InstanceType<'a>),
    /// A resource type.
    Resource(ResourceType<'a>),
}

impl<'a> Parse<'a> for TypeDef<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if parser.peek::<LParen>()? {
            parser.parens(|parser| {
                let mut l = parser.lookahead1();
                if l.peek::<kw::func>()? {
                    parser.parse::<kw::func>()?;
                    Ok(Self::Func(parser.parse()?))
                } else if l.peek::<kw::component>()? {
                    parser.parse::<kw::component>()?;
                    Ok(Self::Component(parser.parse()?))
                } else if l.peek::<kw::instance>()? {
                    parser.parse::<kw::instance>()?;
                    Ok(Self::Instance(parser.parse()?))
                } else if l.peek::<kw::resource>()? {
                    parser.parse::<kw::resource>()?;
                    Ok(Self::Resource(parser.parse()?))
                } else {
                    Ok(Self::Defined(ComponentDefinedType::parse_non_primitive(
                        parser, l,
                    )?))
                }
            })
        } else {
            // Only primitive types have no parens
            Ok(Self::Defined(ComponentDefinedType::Primitive(
                parser.parse()?,
            )))
        }
    }
}

/// A primitive value type.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveValType {
    Bool,
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
    Char,
    String,
    ErrorContext,
}

impl<'a> Parse<'a> for PrimitiveValType {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut l = parser.lookahead1();
        if l.peek::<kw::bool_>()? {
            parser.parse::<kw::bool_>()?;
            Ok(Self::Bool)
        } else if l.peek::<kw::s8>()? {
            parser.parse::<kw::s8>()?;
            Ok(Self::S8)
        } else if l.peek::<kw::u8>()? {
            parser.parse::<kw::u8>()?;
            Ok(Self::U8)
        } else if l.peek::<kw::s16>()? {
            parser.parse::<kw::s16>()?;
            Ok(Self::S16)
        } else if l.peek::<kw::u16>()? {
            parser.parse::<kw::u16>()?;
            Ok(Self::U16)
        } else if l.peek::<kw::s32>()? {
            parser.parse::<kw::s32>()?;
            Ok(Self::S32)
        } else if l.peek::<kw::u32>()? {
            parser.parse::<kw::u32>()?;
            Ok(Self::U32)
        } else if l.peek::<kw::s64>()? {
            parser.parse::<kw::s64>()?;
            Ok(Self::S64)
        } else if l.peek::<kw::u64>()? {
            parser.parse::<kw::u64>()?;
            Ok(Self::U64)
        } else if l.peek::<kw::f32>()? {
            parser.parse::<kw::f32>()?;
            Ok(Self::F32)
        } else if l.peek::<kw::f64>()? {
            parser.parse::<kw::f64>()?;
            Ok(Self::F64)
        } else if l.peek::<kw::float32>()? {
            parser.parse::<kw::float32>()?;
            Ok(Self::F32)
        } else if l.peek::<kw::float64>()? {
            parser.parse::<kw::float64>()?;
            Ok(Self::F64)
        } else if l.peek::<kw::char>()? {
            parser.parse::<kw::char>()?;
            Ok(Self::Char)
        } else if l.peek::<kw::string>()? {
            parser.parse::<kw::string>()?;
            Ok(Self::String)
        } else if l.peek::<kw::error_context>()? {
            parser.parse::<kw::error_context>()?;
            Ok(Self::ErrorContext)
        } else {
            Err(l.error())
        }
    }
}

impl Peek for PrimitiveValType {
    fn peek(cursor: crate::parser::Cursor<'_>) -> Result<bool> {
        Ok(matches!(
            cursor.keyword()?,
            Some(("bool", _))
                | Some(("s8", _))
                | Some(("u8", _))
                | Some(("s16", _))
                | Some(("u16", _))
                | Some(("s32", _))
                | Some(("u32", _))
                | Some(("s64", _))
                | Some(("u64", _))
                | Some(("f32", _))
                | Some(("f64", _))
                | Some(("float32", _))
                | Some(("float64", _))
                | Some(("char", _))
                | Some(("string", _))
                | Some(("error-context", _))
        ))
    }

    fn display() -> &'static str {
        "primitive value type"
    }
}

/// A component value type.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum ComponentValType<'a> {
    /// The value type is an inline defined type.
    Inline(ComponentDefinedType<'a>),
    /// The value type is an index reference to a defined type.
    Ref(Index<'a>),
}

impl<'a> Parse<'a> for ComponentValType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if parser.peek::<Index<'_>>()? {
            Ok(Self::Ref(parser.parse()?))
        } else {
            Ok(Self::Inline(InlineComponentValType::parse(parser)?.0))
        }
    }
}

impl Peek for ComponentValType<'_> {
    fn peek(cursor: crate::parser::Cursor<'_>) -> Result<bool> {
        Ok(Index::peek(cursor)? || ComponentDefinedType::peek(cursor)?)
    }

    fn display() -> &'static str {
        "component value type"
    }
}

/// An inline-only component value type.
///
/// This variation does not parse type indexes.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct InlineComponentValType<'a>(ComponentDefinedType<'a>);

impl<'a> Parse<'a> for InlineComponentValType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if parser.peek::<LParen>()? {
            parser.parens(|parser| {
                Ok(Self(ComponentDefinedType::parse_non_primitive(
                    parser,
                    parser.lookahead1(),
                )?))
            })
        } else {
            Ok(Self(ComponentDefinedType::Primitive(parser.parse()?)))
        }
    }
}

// A component defined type.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum ComponentDefinedType<'a> {
    Primitive(PrimitiveValType),
    Record(Record<'a>),
    Variant(Variant<'a>),
    List(List<'a>),
    Tuple(Tuple<'a>),
    Flags(Flags<'a>),
    Enum(Enum<'a>),
    Option(OptionType<'a>),
    Result(ResultType<'a>),
    Own(Index<'a>),
    Borrow(Index<'a>),
    Stream(Stream<'a>),
    Future(Future<'a>),
}

impl<'a> ComponentDefinedType<'a> {
    fn parse_non_primitive(parser: Parser<'a>, mut l: Lookahead1<'a>) -> Result<Self> {
        parser.depth_check()?;
        if l.peek::<kw::record>()? {
            Ok(Self::Record(parser.parse()?))
        } else if l.peek::<kw::variant>()? {
            Ok(Self::Variant(parser.parse()?))
        } else if l.peek::<kw::list>()? {
            Ok(Self::List(parser.parse()?))
        } else if l.peek::<kw::tuple>()? {
            Ok(Self::Tuple(parser.parse()?))
        } else if l.peek::<kw::flags>()? {
            Ok(Self::Flags(parser.parse()?))
        } else if l.peek::<kw::enum_>()? {
            Ok(Self::Enum(parser.parse()?))
        } else if l.peek::<kw::option>()? {
            Ok(Self::Option(parser.parse()?))
        } else if l.peek::<kw::result>()? {
            Ok(Self::Result(parser.parse()?))
        } else if l.peek::<kw::own>()? {
            parser.parse::<kw::own>()?;
            Ok(Self::Own(parser.parse()?))
        } else if l.peek::<kw::borrow>()? {
            parser.parse::<kw::borrow>()?;
            Ok(Self::Borrow(parser.parse()?))
        } else if l.peek::<kw::stream>()? {
            Ok(Self::Stream(parser.parse()?))
        } else if l.peek::<kw::future>()? {
            Ok(Self::Future(parser.parse()?))
        } else {
            Err(l.error())
        }
    }
}

impl Default for ComponentDefinedType<'_> {
    fn default() -> Self {
        Self::Primitive(PrimitiveValType::Bool)
    }
}

impl Peek for ComponentDefinedType<'_> {
    fn peek(cursor: crate::parser::Cursor<'_>) -> Result<bool> {
        if PrimitiveValType::peek(cursor)? {
            return Ok(true);
        }

        Ok(match cursor.lparen()? {
            Some(cursor) => matches!(
                cursor.keyword()?,
                Some(("record", _))
                    | Some(("variant", _))
                    | Some(("list", _))
                    | Some(("tuple", _))
                    | Some(("flags", _))
                    | Some(("enum", _))
                    | Some(("option", _))
                    | Some(("result", _))
                    | Some(("own", _))
                    | Some(("borrow", _))
            ),
            None => false,
        })
    }

    fn display() -> &'static str {
        "component defined type"
    }
}

/// A record defined type.
#[derive(Debug)]
pub struct Record<'a> {
    /// The fields of the record.
    pub fields: Vec<RecordField<'a>>,
}

impl<'a> Parse<'a> for Record<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::record>()?;
        let mut fields = Vec::new();
        while !parser.is_empty() {
            fields.push(parser.parens(|p| p.parse())?);
        }
        Ok(Self { fields })
    }
}

/// A record type field.
#[derive(Debug)]
pub struct RecordField<'a> {
    /// The name of the field.
    pub name: &'a str,
    /// The type of the field.
    pub ty: ComponentValType<'a>,
}

impl<'a> Parse<'a> for RecordField<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::field>()?;
        Ok(Self {
            name: parser.parse()?,
            ty: parser.parse()?,
        })
    }
}

/// A variant defined type.
#[derive(Debug)]
pub struct Variant<'a> {
    /// The cases of the variant type.
    pub cases: Vec<VariantCase<'a>>,
}

impl<'a> Parse<'a> for Variant<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::variant>()?;
        let mut cases = Vec::new();
        while !parser.is_empty() {
            cases.push(parser.parens(|p| p.parse())?);
        }
        Ok(Self { cases })
    }
}

/// A case of a variant type.
#[derive(Debug)]
pub struct VariantCase<'a> {
    /// Where this `case` was defined
    pub span: Span,
    /// An optional identifier to refer to this case by as part of name
    /// resolution.
    pub id: Option<Id<'a>>,
    /// The name of the case.
    pub name: &'a str,
    /// The optional type of the case.
    pub ty: Option<ComponentValType<'a>>,
    /// The optional refinement.
    pub refines: Option<Refinement<'a>>,
}

impl<'a> Parse<'a> for VariantCase<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let span = parser.parse::<kw::case>()?.0;
        let id = parser.parse()?;
        let name = parser.parse()?;
        let ty = parser.parse()?;
        let refines = if !parser.is_empty() {
            Some(parser.parse()?)
        } else {
            None
        };
        Ok(Self {
            span,
            id,
            name,
            ty,
            refines,
        })
    }
}

/// A refinement for a variant case.
#[derive(Debug)]
pub enum Refinement<'a> {
    /// The refinement is referenced by index.
    Index(Span, Index<'a>),
    /// The refinement has been resolved to an index into
    /// the cases of the variant.
    Resolved(u32),
}

impl<'a> Parse<'a> for Refinement<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parens(|parser| {
            let span = parser.parse::<kw::refines>()?.0;
            let id = parser.parse()?;
            Ok(Self::Index(span, id))
        })
    }
}

/// A list type.
#[derive(Debug)]
pub struct List<'a> {
    /// The element type of the array.
    pub element: Box<ComponentValType<'a>>,
}

impl<'a> Parse<'a> for List<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::list>()?;
        Ok(Self {
            element: Box::new(parser.parse()?),
        })
    }
}

/// A tuple type.
#[derive(Debug)]
pub struct Tuple<'a> {
    /// The types of the fields of the tuple.
    pub fields: Vec<ComponentValType<'a>>,
}

impl<'a> Parse<'a> for Tuple<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::tuple>()?;
        let mut fields = Vec::new();
        while !parser.is_empty() {
            fields.push(parser.parse()?);
        }
        Ok(Self { fields })
    }
}

/// A flags type.
#[derive(Debug)]
pub struct Flags<'a> {
    /// The names of the individual flags.
    pub names: Vec<&'a str>,
}

impl<'a> Parse<'a> for Flags<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::flags>()?;
        let mut names = Vec::new();
        while !parser.is_empty() {
            names.push(parser.parse()?);
        }
        Ok(Self { names })
    }
}

/// An enum type.
#[derive(Debug)]
pub struct Enum<'a> {
    /// The tag names of the enum.
    pub names: Vec<&'a str>,
}

impl<'a> Parse<'a> for Enum<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::enum_>()?;
        let mut names = Vec::new();
        while !parser.is_empty() {
            names.push(parser.parse()?);
        }
        Ok(Self { names })
    }
}

/// An optional type.
#[derive(Debug)]
pub struct OptionType<'a> {
    /// The type of the value, when a value is present.
    pub element: Box<ComponentValType<'a>>,
}

impl<'a> Parse<'a> for OptionType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::option>()?;
        Ok(Self {
            element: Box::new(parser.parse()?),
        })
    }
}

/// A result type.
#[derive(Debug)]
pub struct ResultType<'a> {
    /// The type on success.
    pub ok: Option<Box<ComponentValType<'a>>>,
    /// The type on failure.
    pub err: Option<Box<ComponentValType<'a>>>,
}

impl<'a> Parse<'a> for ResultType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::result>()?;

        let ok: Option<ComponentValType> = parser.parse()?;
        let err: Option<ComponentValType> = if parser.peek::<LParen>()? {
            Some(parser.parens(|parser| {
                parser.parse::<kw::error>()?;
                parser.parse()
            })?)
        } else {
            None
        };

        Ok(Self {
            ok: ok.map(Box::new),
            err: err.map(Box::new),
        })
    }
}

/// A stream type.
#[derive(Debug)]
pub struct Stream<'a> {
    /// The element type of the stream.
    pub element: Option<Box<ComponentValType<'a>>>,
}

impl<'a> Parse<'a> for Stream<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::stream>()?;
        Ok(Self {
            element: parser.parse::<Option<ComponentValType>>()?.map(Box::new),
        })
    }
}

/// A future type.
#[derive(Debug)]
pub struct Future<'a> {
    /// The element type of the future, if any.
    pub element: Option<Box<ComponentValType<'a>>>,
}

impl<'a> Parse<'a> for Future<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::future>()?;
        Ok(Self {
            element: parser.parse::<Option<ComponentValType>>()?.map(Box::new),
        })
    }
}

/// A component function type with parameters and result.
#[derive(Debug)]
pub struct ComponentFunctionType<'a> {
    /// The parameters of a function, optionally each having an identifier for
    /// name resolution and a name for the custom `name` section.
    pub params: Box<[ComponentFunctionParam<'a>]>,
    /// The result of a function.
    pub result: Option<ComponentValType<'a>>,
}

impl<'a> Parse<'a> for ComponentFunctionType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut params: Vec<ComponentFunctionParam> = Vec::new();
        while parser.peek2::<kw::param>()? {
            params.push(parser.parens(|p| p.parse())?);
        }

        let result = if parser.peek2::<kw::result>()? {
            Some(parser.parens(|p| {
                p.parse::<kw::result>()?;
                p.parse()
            })?)
        } else {
            None
        };

        Ok(Self {
            params: params.into(),
            result,
        })
    }
}

/// A parameter of a [`ComponentFunctionType`].
#[derive(Debug)]
pub struct ComponentFunctionParam<'a> {
    /// The name of the parameter
    pub name: &'a str,
    /// The type of the parameter.
    pub ty: ComponentValType<'a>,
}

impl<'a> Parse<'a> for ComponentFunctionParam<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::param>()?;
        Ok(Self {
            name: parser.parse()?,
            ty: parser.parse()?,
        })
    }
}

/// A result of a [`ComponentFunctionType`].
#[derive(Debug)]
pub struct ComponentFunctionResult<'a> {
    /// An optionally-specified name of this result
    pub name: Option<&'a str>,
    /// The type of the result.
    pub ty: ComponentValType<'a>,
}

impl<'a> Parse<'a> for ComponentFunctionResult<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.parse::<kw::result>()?;
        Ok(Self {
            name: parser.parse()?,
            ty: parser.parse()?,
        })
    }
}

/// The type of an exported item from an component or instance type.
#[derive(Debug)]
pub struct ComponentExportType<'a> {
    /// Where this export was defined.
    pub span: Span,
    /// The name of this export.
    pub name: ComponentExternName<'a>,
    /// The signature of the item.
    pub item: ItemSig<'a>,
}

impl<'a> Parse<'a> for ComponentExportType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let span = parser.parse::<kw::export>()?.0;
        let id = parser.parse()?;
        let debug_name = parser.parse()?;
        let name = parser.parse()?;
        let item = parser.parens(|p| {
            let mut item = p.parse::<ItemSigNoName<'_>>()?.0;
            item.id = id;
            item.name = debug_name;
            Ok(item)
        })?;
        Ok(Self { span, name, item })
    }
}

/// A type definition for a component type.
#[derive(Debug, Default)]
pub struct ComponentType<'a> {
    /// The declarations of the component type.
    pub decls: Vec<ComponentTypeDecl<'a>>,
}

impl<'a> Parse<'a> for ComponentType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.depth_check()?;
        Ok(Self {
            decls: parser.parse()?,
        })
    }
}

/// A declaration of a component type.
#[derive(Debug)]
pub enum ComponentTypeDecl<'a> {
    /// A core type definition local to the component type.
    CoreType(CoreType<'a>),
    /// A type definition local to the component type.
    Type(Type<'a>),
    /// An alias local to the component type.
    Alias(Alias<'a>),
    /// An import of the component type.
    Import(ComponentImport<'a>),
    /// An export of the component type.
    Export(ComponentExportType<'a>),
}

impl<'a> Parse<'a> for ComponentTypeDecl<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut l = parser.lookahead1();
        if l.peek::<kw::core>()? {
            Ok(Self::CoreType(parser.parse()?))
        } else if l.peek::<kw::r#type>()? {
            Ok(Self::Type(Type::parse_no_inline_exports(parser)?))
        } else if l.peek::<kw::alias>()? {
            Ok(Self::Alias(parser.parse()?))
        } else if l.peek::<kw::import>()? {
            Ok(Self::Import(parser.parse()?))
        } else if l.peek::<kw::export>()? {
            Ok(Self::Export(parser.parse()?))
        } else {
            Err(l.error())
        }
    }
}

impl<'a> Parse<'a> for Vec<ComponentTypeDecl<'a>> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut decls = Vec::new();
        while !parser.is_empty() {
            decls.push(parser.parens(|parser| parser.parse())?);
        }
        Ok(decls)
    }
}

/// A type definition for an instance type.
#[derive(Debug)]
pub struct InstanceType<'a> {
    /// The declarations of the instance type.
    pub decls: Vec<InstanceTypeDecl<'a>>,
}

impl<'a> Parse<'a> for InstanceType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        parser.depth_check()?;
        Ok(Self {
            decls: parser.parse()?,
        })
    }
}

/// A declaration of an instance type.
#[derive(Debug)]
pub enum InstanceTypeDecl<'a> {
    /// A core type definition local to the component type.
    CoreType(CoreType<'a>),
    /// A type definition local to the instance type.
    Type(Type<'a>),
    /// An alias local to the instance type.
    Alias(Alias<'a>),
    /// An export of the instance type.
    Export(ComponentExportType<'a>),
}

impl<'a> Parse<'a> for InstanceTypeDecl<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut l = parser.lookahead1();
        if l.peek::<kw::core>()? {
            Ok(Self::CoreType(parser.parse()?))
        } else if l.peek::<kw::r#type>()? {
            Ok(Self::Type(Type::parse_no_inline_exports(parser)?))
        } else if l.peek::<kw::alias>()? {
            Ok(Self::Alias(parser.parse()?))
        } else if l.peek::<kw::export>()? {
            Ok(Self::Export(parser.parse()?))
        } else {
            Err(l.error())
        }
    }
}

impl<'a> Parse<'a> for Vec<InstanceTypeDecl<'a>> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut decls = Vec::new();
        while !parser.is_empty() {
            decls.push(parser.parens(|parser| parser.parse())?);
        }
        Ok(decls)
    }
}

/// A type definition for an instance type.
#[derive(Debug)]
pub struct ResourceType<'a> {
    /// Representation, in core WebAssembly, of this resource.
    pub rep: core::ValType<'a>,
    /// The declarations of the instance type.
    pub dtor: Option<CoreItemRef<'a, kw::func>>,
}

impl<'a> Parse<'a> for ResourceType<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let rep = parser.parens(|p| {
            p.parse::<kw::rep>()?;
            p.parse()
        })?;
        let dtor = if parser.is_empty() {
            None
        } else {
            Some(parser.parens(|p| {
                p.parse::<kw::dtor>()?;
                p.parens(|p| p.parse())
            })?)
        };
        Ok(Self { rep, dtor })
    }
}

/// A value type declaration used for values in import signatures.
#[derive(Debug)]
pub struct ComponentValTypeUse<'a>(pub ComponentValType<'a>);

impl<'a> Parse<'a> for ComponentValTypeUse<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        match ComponentTypeUse::<'a, InlineComponentValType<'a>>::parse(parser)? {
            ComponentTypeUse::Ref(i) => Ok(Self(ComponentValType::Ref(i.idx))),
            ComponentTypeUse::Inline(t) => Ok(Self(ComponentValType::Inline(t.0))),
        }
    }
}

/// A reference to a core type defined in this component.
///
/// This is the same as `TypeUse`, but accepts `$T` as shorthand for
/// `(type $T)`.
#[derive(Debug, Clone)]
pub enum CoreTypeUse<'a, T> {
    /// The type that we're referencing.
    Ref(CoreItemRef<'a, kw::r#type>),
    /// The inline type.
    Inline(T),
}

impl<'a, T: Parse<'a>> Parse<'a> for CoreTypeUse<'a, T> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        // Here the core context is assumed, so no core prefix is expected
        if parser.peek::<LParen>()? && parser.peek2::<CoreItemRef<'a, kw::r#type>>()? {
            Ok(Self::Ref(parser.parens(|parser| parser.parse())?))
        } else {
            Ok(Self::Inline(parser.parse()?))
        }
    }
}

impl<T> Default for CoreTypeUse<'_, T> {
    fn default() -> Self {
        let span = Span::from_offset(0);
        Self::Ref(CoreItemRef {
            idx: Index::Num(0, span),
            kind: kw::r#type(span),
            export_name: None,
        })
    }
}

/// A reference to a type defined in this component.
///
/// This is the same as `TypeUse`, but accepts `$T` as shorthand for
/// `(type $T)`.
#[derive(Debug, Clone)]
pub enum ComponentTypeUse<'a, T> {
    /// The type that we're referencing.
    Ref(ItemRef<'a, kw::r#type>),
    /// The inline type.
    Inline(T),
}

impl<'a, T: Parse<'a>> Parse<'a> for ComponentTypeUse<'a, T> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        if parser.peek::<LParen>()? && parser.peek2::<ItemRef<'a, kw::r#type>>()? {
            Ok(Self::Ref(parser.parens(|parser| parser.parse())?))
        } else {
            Ok(Self::Inline(parser.parse()?))
        }
    }
}

impl<T> Default for ComponentTypeUse<'_, T> {
    fn default() -> Self {
        let span = Span::from_offset(0);
        Self::Ref(ItemRef {
            idx: Index::Num(0, span),
            kind: kw::r#type(span),
            export_names: Vec::new(),
        })
    }
}
