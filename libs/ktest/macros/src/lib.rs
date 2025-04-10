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
                    module: module_path!(),
                    name: stringify!(#ident),
                    ignored: false
                }
            }
        };

        #func
    )
    .into()
}

// #[ktest(crate = path::to::ktest)]
pub(crate) fn crate_path(attrs: &mut Vec<Attribute>) -> syn::Result<Path> {
    let mut crate_path = None;
    let mut errors: Option<Error> = None;

    attrs.retain(|attr| {
        if !attr.path().is_ident("ktest") {
            return true;
        }
        if let Err(err) = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                if crate_path.is_some() {
                    return Err(meta.error("duplicate ktest crate attribute"));
                }
                let path = meta.value()?.call(Path::parse_mod_style)?;
                crate_path = Some(path);
                Ok(())
            } else {
                Err(meta.error("unsupported ktest attribute"))
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
        None => Ok(crate_path.unwrap_or_else(|| parse_quote!(::ktest))),
        Some(errors) => Err(errors),
    }
}

// #[derive(Debug)]
// struct ForEachFixtureInput {
//     folder: PathBuf,
//     macroo: Ident,
// }
//
// impl Parse for ForEachFixtureInput {
//     fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
//         let folder: LitStr = input.parse()?;
//         input.parse::<syn::Token![,]>()?;
//         let macroo = input.parse()?;
//         Ok(Self {
//             folder: parse_path(&folder),
//             macroo,
//         })
//     }
// }

// fn parse_path(path: &syn::LitStr) -> PathBuf {
//     let path = path.value();
//     let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
//     manifest_dir.join(path)
// }

// #[proc_macro]
// pub fn for_each_fixture(input: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(input as ForEachFixtureInput);
//
//     let folder = fs::read_dir(input.folder).unwrap();
//
//     let cases = folder.filter_map(|entry| {
//         let entry = entry.unwrap();
//         let is_wasm = if let Some(ext) = entry.path().extension() {
//             ext == "wasm" || ext == "wast"
//         } else {
//             false
//         };
//
//         if is_wasm {
//             let path = entry.path();
//             let name = format_ident!(
//                 "test_{}",
//                 path.file_stem()
//                     .unwrap()
//                     .to_str()
//                     .unwrap()
//                     .replace("-", "_")
//             );
//             let path = path.to_str().unwrap();
//             let macroo = &input.macroo;
//             Some(quote! {
//                 #macroo!(#name, #path);
//             })
//         } else {
//             None
//         }
//     });
//
//     quote!(#(#cases)*).into()
// }
