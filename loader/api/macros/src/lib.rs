use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, parse_quote, Attribute, Error, Expr, ItemFn, Path};

/// Marks the function as the entry point of the program.
///
/// After setting up the environment the bootloader will call this function
#[allow(clippy::missing_panics_doc)] // This is a macro
#[proc_macro_attribute]
pub fn entry(args: TokenStream, item: TokenStream) -> TokenStream {
    let expr = parse_macro_input!(args as Expr);
    let mut func = parse_macro_input!(item as ItemFn);

    let crate_path = crate_path(&mut func.attrs).unwrap();
    let func_ident = &func.sig.ident;

    quote!(
        #[used(linker)]
        #[link_section = ".loader_config"]
        static __LOADER_CONFIG: #crate_path::LoaderConfig = {
            let config: #crate_path::LoaderConfig = #expr;
            config
        };

        #[no_mangle]
        #[export_name = "_start"]
        pub extern "C" fn __start_impl(hartid: usize, boot_info: &'static mut #crate_path::BootInfo) -> ! {
            // validate the signature of the program entry point
            let f: fn(usize, &'static mut #crate_path::BootInfo) -> ! = #func_ident;

            f(hartid, boot_info)
        }

        #func
    )
    .into()
}

// #[loader_api(crate = path::to::loader_api)]
pub(crate) fn crate_path(attrs: &mut Vec<Attribute>) -> syn::Result<Path> {
    let mut crate_path = None;
    let mut errors: Option<Error> = None;

    attrs.retain(|attr| {
        if !attr.path().is_ident("loader_api") {
            return true;
        }
        if let Err(err) = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                if crate_path.is_some() {
                    return Err(meta.error("duplicate loader_api crate attribute"));
                }
                let path = meta.value()?.call(Path::parse_mod_style)?;
                crate_path = Some(path);
                Ok(())
            } else {
                Err(meta.error("unsupported loader_api attribute"))
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
        None => Ok(crate_path.unwrap_or_else(|| parse_quote!(::loader_api))),
        Some(errors) => Err(errors),
    }
}
