// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
// use std::{fs, path::PathBuf};
use syn::{Attribute, Error, ItemFn, Path, parse_macro_input, parse_quote};

#[proc_macro_attribute]
pub fn test(_args: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);

    let ident = &func.sig.ident;
    let ident_str = ident.to_string();
    let static_ident = format_ident!("__K23_TEST_{}", ident_str.to_uppercase());
    let crate_path = crate_path(&mut func.attrs).unwrap();

    quote!(
        #[used(linker)]
        #[unsafe(link_section = "k23_tests")]
        static #static_ident: #crate_path::Test = {
            #crate_path::Test {
                run: || ::alloc::boxed::Box::pin(#ident()),
                info: #crate_path::TestInfo {
                    ident: concat!(module_path!(), "::", stringify!(#ident)),
                    ignored: false
                }
            }
        };

        #func
    )
    .into()
}

// #[test(crate = path::to::test)]
pub(crate) fn crate_path(attrs: &mut Vec<Attribute>) -> syn::Result<Path> {
    let mut crate_path = None;
    let mut errors: Option<Error> = None;

    attrs.retain(|attr| {
        if !attr.path().is_ident("test") {
            return true;
        }
        if let Err(err) = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                if crate_path.is_some() {
                    return Err(meta.error("duplicate test crate attribute"));
                }
                let path = meta.value()?.call(Path::parse_mod_style)?;
                crate_path = Some(path);
                Ok(())
            } else {
                Err(meta.error("unsupported test attribute"))
            }
        }) {
            match &mut errors {
                None => errors = Some(err),
                Some(errors) => errors.combine(err),
            }
        }
        false
    });

    match errors {
        None => Ok(crate_path.unwrap_or_else(|| parse_quote!(::test))),
        Some(errors) => Err(errors),
    }
}
