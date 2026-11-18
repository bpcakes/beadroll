//! The `hardening` subject's tests of the core: dangling dependencies and their retraction, whatever
//! order concurrent events and journals arrive in.

mod dangling_replay_determinism;
mod key_retraction_survives_a_concurrent_target;
