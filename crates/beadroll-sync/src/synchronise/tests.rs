use super::{ROUNDS, longest_pause};

/// B02: the bound doubles from 100 ms, then stays at two seconds for every remaining round.
#[test]
fn pause_bound_doubles_then_stays_capped() {
    for (attempt, expected_ms) in [(1, 100), (2, 200), (3, 400), (4, 800), (5, 1600)] {
        let bound = std::time::Duration::from_secs_f64(longest_pause(attempt));
        assert_eq!(bound, std::time::Duration::from_millis(expected_ms));
    }
    for attempt in 6..=ROUNDS {
        assert_eq!(longest_pause(attempt), 2.0, "attempt {attempt}");
    }
}

/// B02: the public pause's zero and extreme attempt inputs retain their bounded calculation.
#[test]
fn pause_bound_handles_zero_and_extreme_attempts() {
    assert_eq!(longest_pause(0), longest_pause(1));
    for attempt in [24, 32, 33, u32::MAX] {
        assert_eq!(longest_pause(attempt), 2.0, "attempt {attempt}");
    }
}
