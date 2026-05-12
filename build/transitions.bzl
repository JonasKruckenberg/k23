# Configuration transition that layers `constraint_values` onto the incoming
# platform and derives a `cfg:` label reflecting the post-transition state.

_NAMED_SETTINGS = [
    "prelude//os/constraints:os",
    "prelude//cpu/constraints:cpu",
    "constraints//:env",
    "constraints//:rust-std",
    "constraints//:sanitizer",
    "constraints//:opt-level",
]

def _cfg_name(cfg: ConfigurationInfo) -> str:
    by_setting = {str(setting): value for setting, value in cfg.constraints.items()}
    parts = []
    for setting in _NAMED_SETTINGS:
        if setting in by_setting:
            label = by_setting[setting].label
            # `constraint` values like `env[host]` carry the variant in sub_target.
            parts.append(label.sub_target[0] if label.sub_target else label.name)
    return "cfg:" + "-".join(parts) if parts else "cfg:<empty>"

def _configuration_transition_impl(ctx: AnalysisContext) -> list[Provider]:
    override_constraints = {}
    for dep in ctx.attrs.constraint_values:
        for label, value in dep[ConfigurationInfo].constraints.items():
            override_constraints[label] = value

    def transition_impl(platform: PlatformInfo) -> PlatformInfo:
        constraints = dict(platform.configuration.constraints)
        for label, value in override_constraints.items():
            constraints[label] = value

        new_cfg = ConfigurationInfo(
            constraints = constraints,
            values = platform.configuration.values,
        )
        return PlatformInfo(label = _cfg_name(new_cfg), configuration = new_cfg)

    return [
        DefaultInfo(),
        TransitionInfo(impl = transition_impl),
    ]

configuration_transition = rule(
    impl = _configuration_transition_impl,
    attrs = {
        "constraint_values": attrs.set(attrs.configuration_label()),
    },
    is_configuration_rule = True,
)
