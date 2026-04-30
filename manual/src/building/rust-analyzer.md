# IDE Setup with rust-analyzer

> Skeleton — bullets to expand into prose.

Since we use buck2 to build k23 instead of Cargo, rust-analyzer has nothing to work with by default.

buck2 ships a companion tool called `rust-project` that walks the buck2 target graph and emits a `rust-project.json`
file that will automatically load. We provide a convenient `just rust-project` command for generating this file.

You should run this command periodicially, at least whenever you added or removed a crate or third-party dependency. It should be relatively straightforward to notice though: when autocompletion is broken re-running `just rust-project` is in order.

All IDEs using rust-analyzer should pick up on this file automatically. If not, raise an issue please.
