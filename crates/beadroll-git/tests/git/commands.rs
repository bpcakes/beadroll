//! `tests/test_commands.py`: the halves of its tests that check beadroll-git alone.

mod sync_repo_damage {
    use beadroll_git::local_damage;

    /// What counts as local damage: git's own read errors, never a line the remote relayed (the
    /// classifier half; the CLI half drives `claim` through a damaged sync.git).
    #[test]
    fn a_claim_rebuilds_too_and_damage_twice_in_one_command_is_reported() {
        assert!(local_damage(
            "error: object file objects/7d/04c9 is empty\nfatal: fetch-pack: invalid index-pack output"
        ));
        assert!(local_damage(
            "fatal: missing object 7d04c9b4c3031216fa66cefa4cf7e3b79212bb32 for refs/beads/remote"
        ));
        assert!(!local_damage(
            "remote: error: object file objects/7d/04c9 is empty\nremote: fatal: bad object"
        ));
        assert!(!local_damage("ssh: Could not resolve hostname github.com"));
    }
}
