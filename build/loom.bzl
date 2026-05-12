load("@prelude//platforms:defs.bzl", "host_configuration")

_DEFAULT_MODIFIERS = [
    "constraints//:opt-level[3]",
]

_DEFAULT_ENV = {
    "LOOM_LOG": "debug",
    "LOOM_MAX_PREEMPTIONS": "2",
    "LOOM_LOCATION": "true",
}

def rust_loom_test(
        name,
        srcs,
        crate,
        deps = [],
        env = {},
        rustc_flags = [],
        modifiers = [],
        labels = [],
        visibility = None,
        **kwargs):
    merged_env = dict(_DEFAULT_ENV)
    merged_env["LOOM_CHECKPOINT_FILE"] = "loom-checkpoint-{}.json".format(name)
    merged_env.update(env)

    native.rust_test(
        name = name,
        srcs = srcs,
        crate = crate,
        deps = deps,
        rustc_flags = ["--cfg=loom"] + rustc_flags,
        target_compatible_with = [host_configuration.os, host_configuration.cpu],
        modifiers = _DEFAULT_MODIFIERS + modifiers,
        labels = ["loom"] + labels,
        env = merged_env,
        visibility = visibility,
        **kwargs
    )
