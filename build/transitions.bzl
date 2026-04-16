# A generic configuration transition rule that overrides specific constraint
# values while preserving all other configuration from the incoming platform.

def _configuration_transition_impl(ctx: AnalysisContext) -> list[Provider]:
    override_constraints = {}
    for dep in ctx.attrs.constraint_values:
        for label, value in dep[ConfigurationInfo].constraints.items():
            override_constraints[label] = value

    def transition_impl(platform: PlatformInfo) -> PlatformInfo:
        constraints = dict(platform.configuration.constraints)
        for label, value in override_constraints.items():
            constraints[label] = value

        return PlatformInfo(
            label = platform.label,
            configuration = ConfigurationInfo(
                constraints = constraints,
                values = platform.configuration.values,
            ),
        )

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
