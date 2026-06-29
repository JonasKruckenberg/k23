# Workspace clippy lint policy. Mirrors the [workspace.lints.clippy] table that
# lived in the root Cargo.toml before the buck2 migration. Wired into the rust
# toolchain via deny_lints / deny_on_check_lints / allow_lints in
# build/toolchains/BUCK.
#
# DENY  — correctness / soundness lints. Hard error on every build, including
#         binaries and tests; we never want a kernel image to ship with one of
#         these warnings silently capped.
# DENY_ON_CHECK — style and refactor-cleanup lints. Warnings on normal builds
#         so they don't break the inner dev loop, but errors on `buck check`
#         / clippy subtargets and in CI so they block landing.

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
    "clippy::cast_slice_from_raw_parts",
    # Casting a function item to an integer is almost always a bug — except in
    # trap-vector / branch-target setup, where the call site explicitly allows
    # it with a `reason`.
    "function_casts_as_integer",

    # stack overflow prevention
    "clippy::large_futures",
    "clippy::large_stack_arrays",
    "clippy::large_stack_frames",
    "clippy::large_types_passed_by_value",
    "clippy::recursive_format_impl",

    # unsafe / api discipline
    "unsafe_op_in_unsafe_fn",
    "clippy::undocumented_unsafe_blocks",
    "clippy::as_underscore",
    "clippy::alloc_instead_of_core",
    "clippy::allow_attributes_without_reason",
    "clippy::fn_params_excessive_bools",
    "clippy::struct_excessive_bools",
    "clippy::missing_fields_in_debug",
    "clippy::trivially_copy_pass_by_ref",
    "unused_unsafe",

    # hygiene
    "unused_features",
    "stable_features",
    "unfulfilled_lint_expectations",
    "deprecated",

    # correctness
    "clippy::if_same_then_else",
]

DENY_ON_CHECK = [
    # rustc unused-family — fires constantly mid-refactor
    "unused_imports",
    "unused_variables",
    "unused_mut",
    "unused_assignments",
    "unused_macros",
    "unused_labels",
    "unused_must_use",
    "unreachable_code",

    # clippy refactor leftovers
    "clippy::needless_return",
    "clippy::redundant_clone",
    "clippy::needless_borrow",
    "clippy::useless_conversion",
    "clippy::redundant_closure",
    "clippy::redundant_closure_for_method_calls",
    "clippy::redundant_field_names",
    "clippy::needless_collect",
    "clippy::question_mark",
    "clippy::redundant_pattern_matching",
    "clippy::clone_on_copy",
    "clippy::unnecessary_mut_passed",
    "clippy::manual_clear",
    "clippy::useless_format",
    "clippy::collapsible_match",
    "clippy::into_iter_on_ref",
    "clippy::only_used_in_recursion",
    "clippy::needless_maybe_sized",
    "clippy::legacy_numeric_constants",
    "clippy::doc_overindented_list_items",
    # Idle loops in scheduler / panic halt paths legitimately spin; those sites
    # local-allow with reason.
    "clippy::empty_loop",

    # "you forgot to remove debugging"
    "clippy::dbg_macro",
    "clippy::print_stdout",
    "clippy::print_stderr",

    # style / idiom
    "clippy::default_trait_access",
    "clippy::cloned_instead_of_copied",
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
    "clippy::semicolon_if_nothing_returned",
    "clippy::unnecessary_wraps",
    "clippy::unnested_or_patterns",

    # docs
    "clippy::missing_panics_doc",
    "clippy::missing_errors_doc",

    "clippy::disallowed_types",
]

ALLOW = [
    "clippy::too_many_arguments",
]
