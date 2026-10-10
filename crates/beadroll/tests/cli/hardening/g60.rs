//! G60: U+061C ARABIC LETTER MARK, the one Unicode `Bidi_Control` character that was missing from
//! the controls bead escapes, is escaped as `؜` in every human output.

mod g60_arabic_letter_mark_escaped {
    use beadroll_test_support::Machine;

    /// G60: U+061C ARABIC LETTER MARK, the one Unicode `Bidi_Control` character that was missing
    /// from the controls bead escapes, is escaped in `create`, `list`, `show` and error output, as
    /// U+200F beside it is, instead of reaching the terminal raw.
    #[test]
    fn arabic_letter_mark_is_escaped_like_the_other_bidi_controls() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "bidi");
        machine.bead(["init", "--prefix", "ex"]);
        // ARABIC LETTER MARK, RIGHT-TO-LEFT MARK
        let (alm, rlm) = ("\u{061c}", "\u{200f}");
        let created = machine.run([
            "create",
            &format!("alm{alm}mark rlm{rlm}mark"),
            "-d",
            &format!("desc{alm}d"),
        ]);
        assert_eq!(created.returncode(), 0, "{}", created.stderr);
        let words: Vec<&str> = created.stdout.split_whitespace().collect();
        let key = words.get(2).expect("a third word").trim_end_matches(':');
        let outputs = [
            ("create", created.stdout.clone()),
            ("list", machine.run(["list"]).stdout),
            ("show", machine.run(["show", key]).stdout),
            (
                "error",
                machine.run(["show", &format!("ex-{alm}zz")]).stderr,
            ),
        ];
        for (name, output) in &outputs {
            assert!(!output.trim().is_empty(), "{name} printed nothing");
            // the neighbouring bidi control is escaped today
            assert!(!output.contains(rlm), "{name}: {output:?}");
            assert!(
                !output.contains(alm),
                "{name} printed U+061C raw instead of as \\u061c: {output:?}"
            );
            assert!(output.contains("\\u061c"), "{name}: {output:?}");
        }
    }
}
