//! `test_protocol`'s in-crate API tests: an ownership attempt whose push failed, was refused, timed
//! out or lost its acknowledgement settles one way or the other, never twice and never half.

mod fences_and_confirmation;
mod retried_release;
mod unpublished_claims;
