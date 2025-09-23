# `fastrand`

Small, very fast, non-cryptographic random number generator.

This is used in places where you need to perform inconsequential randomization, such as picking a scheduler worker to steal
tasks from, backoff randomization etc.