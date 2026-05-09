# Workspace clippy lint policy. Mirrors the [workspace.lints.clippy] table that
# lived in the root Cargo.toml before the buck2 migration. Wired into the rust
# toolchain via deny_lints / allow_lints in build/toolchains/BUCK.

DENY = [
    # numeric safety
    "clippy::cast_possible_truncation",
    "clippy::cast_possible_wrap",
    "clippy::cast_precision_loss",
    "clippy::cast_sign_loss",
    "clippy::cast_lossless",
    "clippy::default_numeric_fallback",
    "clippy::checked_conversions",
    "clippy::float_arithmetic",
    "clippy::float_cmp",

    # pointer safety
    "clippy::cast_ptr_alignment",
    "clippy::ptr_as_ptr",
    "clippy::ptr_cast_constness",
    "clippy::ref_as_ptr",
    "clippy::transmute_ptr_to_ptr",

    # stack overflow prevention
    "clippy::large_futures",
    "clippy::large_stack_arrays",
    "clippy::large_stack_frames",
    "clippy::large_types_passed_by_value",
    "clippy::recursive_format_impl",

    # style
    "clippy::undocumented_unsafe_blocks",
    "clippy::as_underscore",
    "clippy::alloc_instead_of_core",
    "clippy::allow_attributes_without_reason",
    "clippy::default_trait_access",
    "clippy::cloned_instead_of_copied",
    "clippy::fn_params_excessive_bools",
    "clippy::struct_excessive_bools",
    "clippy::filter_map_next",
    "clippy::explicit_iter_loop",
    "clippy::flat_map_option",
    "clippy::iter_filter_is_ok",
    "clippy::iter_filter_is_some",
    "clippy::manual_assert",
    "clippy::manual_is_power_of_two",
    "clippy::manual_is_variant_and",
    "clippy::manual_let_else",
    "clippy::manual_ok_or",
    "clippy::match_bool",
    "clippy::missing_fields_in_debug",
    "clippy::semicolon_if_nothing_returned",
    "clippy::trivially_copy_pass_by_ref",
    "clippy::unnecessary_wraps",
    "clippy::unnested_or_patterns",

    # docs
    "clippy::missing_panics_doc",
    "clippy::missing_errors_doc",
]

ALLOW = [
    "clippy::too_many_arguments",
]
