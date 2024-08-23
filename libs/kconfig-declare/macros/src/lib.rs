#![allow(unused)]

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::{format_ident, quote, ToTokens};
use std::path::PathBuf;
use std::sync::OnceLock;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{token, ItemConst, LitStr, Meta, Token};

#[proc_macro_attribute]
pub fn symbol(args: TokenStream, input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::ItemConst);
    let args = syn::parse_macro_input!(args as Args);

    let (config, include) = parse_from_env();

    let value = if let Ok((_, value)) = args.lookup_value(config) {
        toml_to_tokens(value)
    } else {
        let def = &input.expr;
        quote!(#def)
    };
    let value = if let Some(parse_fn) = args.parse_fn() {
        quote! {#parse_fn(#value)}
    } else {
        value
    };

    #[cfg(feature = "collect-symbols")]
    let collect_symbol = collect_symbol(&input, &args);
    #[cfg(not(feature = "collect-symbols"))]
    let collect_symbol = quote! {};

    let attrs = input.attrs;
    let vis = input.vis;
    let ident = input.ident;
    let ty = input.ty;

    quote! (
        #collect_symbol

        #(#attrs)*
        #vis const #ident: #ty = {
            #include
            #value
        };
    )
    .into()
}

fn toml_to_tokens(value: &toml::Value) -> proc_macro2::TokenStream {
    match value {
        toml::Value::Boolean(v) => {
            quote! { #v}
        }
        toml::Value::Integer(v) => {
            let v = Literal::i64_unsuffixed(*v);
            quote! {#v}
        }
        toml::Value::Float(v) => {
            let v = Literal::f64_unsuffixed(*v);
            quote! {#v}
        }
        toml::Value::String(v) => {
            quote! {#v}
        }
        toml::Value::Array(v) => {
            let v = v.iter().map(toml_to_tokens);

            quote! {&[#(#v,)*]}
        }
        _ => unimplemented!(),
    }
}

#[cfg(feature = "collect-symbols")]
fn collect_symbol(input: &ItemConst, args: &Args) -> proc_macro2::TokenStream {
    let static_ident = format_ident!("__{}", input.ident);
    let ident_str = input.ident.to_string();
    let paths = args.paths();

    let doc_comments = input.attrs.iter().filter_map(|attr| {
        if let Meta::NameValue(nv) = &attr.meta {
            if nv.path.require_ident().unwrap().to_string() == "doc" {
                Some(nv.value.to_token_stream())
            } else {
                None
            }
        } else {
            None
        }
    });

    quote! {
        #[::kconfig::linkme::distributed_slice(::kconfig::ITEMS)]
        static #static_ident: ::kconfig::Item = ::kconfig::Item {
            name: #ident_str,
            paths: &[#(#paths,)*],
            description: &[#(#doc_comments,)*]
        };
    }
}

enum Args {
    Paths(Paths),
    Config(Config),
}

impl Args {
    pub fn lookup_value<'a>(
        &self,
        table: &'a toml::Value,
    ) -> Result<(String, &'a toml::Value), String> {
        fn access_by_path<'a, 'b>(
            mut value: &'a toml::Value,
            path: impl IntoIterator<Item = &'b str>,
        ) -> Option<&'a toml::Value> {
            for key in path {
                value = value.as_table()?.get(key)?;
            }

            Some(value)
        }

        for path in self.paths() {
            if let Some(value) = access_by_path(table, path.split('.')) {
                return Ok((path, value));
            }
        }

        Err(format!(
            "no config value found for paths `{}`",
            self.paths().collect::<Vec<_>>().join(", ")
        ))
    }

    fn paths(&self) -> Box<dyn Iterator<Item = String> + '_> {
        match self {
            Self::Paths(paths) => Box::new(paths.0.iter().map(|path| path.value())),
            Self::Config(config) => Box::new(config.paths.0.iter().map(|path| path.value())),
        }
    }

    fn parse_fn(&self) -> Option<&syn::Ident> {
        match self {
            Self::Paths(_) => None,
            Self::Config(config) => config.parse.as_ref(),
        }
    }
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let l = input.lookahead1();

        if l.peek(token::Brace) {
            let config = Config::parse(input)?;

            Ok(Self::Config(config))
        } else {
            let paths = input.parse::<Paths>()?;

            Ok(Self::Paths(paths))
        }
    }
}

struct Config {
    paths: Paths,
    parse: Option<syn::Ident>,
}

impl Parse for Config {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let call_site = proc_macro2::Span::call_site();

        let mut paths = None;
        let mut parse = None;

        let content;
        syn::braced!(content in input);
        let fields = Punctuated::<ConfigField, Token![,]>::parse_terminated(&content)?;
        for field in fields.into_pairs() {
            match field.into_value() {
                ConfigField::Paths(v) => {
                    if paths.is_some() {
                        return Err(syn::Error::new(call_site, "duplicate `paths` field"));
                    }

                    paths = Some(v);
                }
                ConfigField::Parse(v) => {
                    if parse.is_some() {
                        return Err(syn::Error::new(call_site, "duplicate `default` field"));
                    }

                    parse = Some(v);
                }
            }
        }

        Ok(Self {
            paths: paths.ok_or_else(|| syn::Error::new(call_site, "missing `paths` field"))?,
            parse,
        })
    }
}

mod kw {
    syn::custom_keyword!(paths);
    syn::custom_keyword!(parse);
}

enum ConfigField {
    Paths(Paths),
    Parse(syn::Ident),
}

impl Parse for ConfigField {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let l = input.lookahead1();
        if l.peek(kw::paths) {
            input.parse::<kw::paths>()?;
            input.parse::<Token![:]>()?;

            let content;
            syn::bracketed!(content in input);

            Ok(ConfigField::Paths(content.parse()?))
        } else if l.peek(kw::parse) {
            input.parse::<kw::parse>()?;
            input.parse::<Token![:]>()?;
            Ok(ConfigField::Parse(input.parse()?))
        } else {
            Err(l.error())
        }
    }
}

struct Paths(Punctuated<LitStr, Token![,]>);

impl Parse for Paths {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(Self(Punctuated::<LitStr, Token![,]>::parse_terminated(
            input,
        )?))
    }
}

fn parse_from_env() -> (&'static toml::Value, Option<proc_macro2::TokenStream>) {
    static CONFIG: OnceLock<toml::Value> = OnceLock::new();

    let workspace_root = PathBuf::from(std::env::var("CARGO_RUSTC_CURRENT_DIR").unwrap());
    let path = workspace_root.join(std::env::var("K23_CONFIG").unwrap());

    let mut include = None;

    let config = CONFIG.get_or_init(|| {
        let filepath = path.to_string_lossy();
        include = Some(quote! {const _: &str = include_str!(#filepath);});

        let content = std::fs::read_to_string(&path).unwrap();
        toml::from_str(&content).unwrap()
    });

    (config, include)
}
