//! `test_protocol`'s in-crate API tests: an ownership attempt whose push failed, was refused, timed
//! out or lost its acknowledgement settles one way or the other, never twice and never half.

mod protocol3;
mod protocol_reaudit;
mod protocol_review4;
