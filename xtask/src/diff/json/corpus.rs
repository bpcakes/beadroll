//! What `diff json` reads: inputs at every edge CPython's scanner and bead's rules draw, and random
//! values written with random spacing, escapes and damage.

use super::JsonCase;
use crate::diff::generate::Tape;

fn bytes(text: &str) -> JsonCase {
    JsonCase::Bytes(text.as_bytes().to_vec())
}

/// Texts at the edges of the scanner: literals, numbers, strings and escapes, structures,
/// whitespace and nesting.
const TEXTS: [&str; 136] = [
    "",
    " ",
    "\n",
    "\r\n",
    "null",
    "true",
    "false",
    "nul",
    "tru",
    "fals",
    "nulll",
    "True",
    "None",
    "NaN",
    "-NaN",
    "Infinity",
    "-Infinity",
    "+Infinity",
    "Infinityx",
    "infinity",
    "[NaN]",
    "{\"a\":-Infinity}",
    "0",
    "-0",
    "00",
    "01",
    "-01",
    "1.",
    ".5",
    "-.5",
    "1.0",
    "1e5",
    "1E5",
    "1e+5",
    "1e-5",
    "1e",
    "1e+",
    "-",
    "+1",
    "0x10",
    "1_000",
    "1.5e300",
    "1e308",
    "1.7976931348623157e308",
    "1.8e308",
    "1e400",
    "-1e400",
    "1E400",
    "1e-400",
    "5e-324",
    "2.5e-324",
    "0.0001",
    "0.00001",
    "123456789012345678901234567890",
    "-9223372036854775808",
    "9223372036854775807",
    "9223372036854775808",
    "18446744073709551616",
    "-0.0",
    "0e0",
    "-0e-0",
    "1.5E+16",
    "1e16",
    "1e15",
    "123.456e-7",
    "4.35",
    "0.1",
    "1/3",
    "1e9999999999999999999",
    "100000000000000000000.0",
    "\"\"",
    "\"a\"",
    "\"\\u0041\"",
    "\"\\u00e9\"",
    "\"\\uD83D\\uDE00\"",
    "\"\\ud83d\\ude00\"",
    "\"\\ud800\"",
    "\"\\udc00\"",
    "\"\\ud800\\ud800\"",
    "\"\\udc00\\ud800\"",
    "\"\\ud800x\"",
    "\"\\ud800\\u0041\"",
    "\"\\u\"",
    "\"\\u12\"",
    "\"\\u12345\"",
    "\"\\uzzzz\"",
    "\"\\x41\"",
    "\"\\a\"",
    "\"\\/\"",
    "\"\\b\\f\\n\\r\\t\"",
    "\"\t\"",
    "\"\u{0}\"",
    "\"\u{1f}\"",
    "\"\u{7f}\"",
    "\"abc",
    "\"abc\\",
    "\"abc\\\"",
    "\"é😀\"",
    "[1,]",
    "[,1]",
    "[1 2]",
    "{\"a\":1,}",
    "{\"a\"}",
    "{\"a\" 1}",
    "{a:1}",
    "{\"a\":1 \"b\":2}",
    "{1:2}",
    "[",
    "{",
    "]",
    "}",
    "[]]",
    "{}}",
    "[] []",
    "{\"a\":1}{}",
    "{\"a\":1,\"a\":2}",
    "{\"a\":1,\"\\u0061\":2,\"b\":3}",
    "{\"\\ud800\":1,\"\\udbff\":2}",
    "{\"\\ud800\":1,\"\\ud800\":2}",
    "{\"k\":\"\\ud800\",\"k\":\"x\"}",
    "{\"k\":\"x\",\"k\":\"\\ud800\"}",
    " \t\n\r[1]\n",
    "\u{b}[1]",
    "\u{a0}[1]",
    "\u{2028}[1]",
    "[1]\n\n",
    "/*c*/1",
    "1 //c",
    "'a'",
    "(1)",
    "[1,\n2,\r\n3]",
    "{\"a\" : [ 1 , { \"b\" :null } ] }",
    "\"\u{2028}\u{85}\"",
    "[\"\\u0000\",\"\\u001f\",\"\\u007f\",\"\\u0080\",\"\\u00ff\",\"\\u0100\",\"\\uffff\"]",
    "[\"\u{e000}\",\"\u{fffd}\",\"\u{10ffff}\"]",
    "[1.0,1e22,1e-7,123456789.0,0.30000000000000004]",
];

/// Bytes that are not UTF-8, or are UTF-8 with a byte-order mark, or UTF-16 and UTF-32.
const RAW: [&[u8]; 16] = [
    b"\"\xed\xa0\x80\"",
    b"\"\xed\xb0\x80\"",
    b"\"\xed\xa0\xbd\xed\xb8\x80\"",
    b"\"\xed\x9f\xbf\"",
    b"\"\xee\x80\x80\"",
    b"\"\xc0\x80\"",
    b"\"\xf5\x80\x80\x80\"",
    b"\"\xe2\x82\"",
    b"\"\xff\"",
    b"\xef\xbb\xbf[1]",
    b"\xef\xbb\xbf",
    b"\xff\xfe[\x001\x00]\x00",
    b"[\x001\x00]\x00",
    b"\x00[\x001\x00]",
    b"\x00\x00\x00[\x00\x00\x001\x00\x00\x00]",
    b"\"\xed\xa0\x80\",\"x\"",
];

/// Every edge, once.
pub(super) fn all() -> Vec<JsonCase> {
    let mut out: Vec<JsonCase> = TEXTS.into_iter().map(bytes).collect();
    out.extend(RAW.iter().map(|b| JsonCase::Bytes(b.to_vec())));
    for (sign, digits) in [
        ("", 4299),
        ("", 4300),
        ("", 4301),
        ("-", 4300),
        ("-", 4301),
        ("-", 4299),
    ] {
        out.push(bytes(&format!("{sign}{}", "7".repeat(digits))));
        out.push(bytes(&format!("[{sign}{},1]", "1".repeat(digits))));
        out.push(bytes(&format!("{sign}{}.5", "7".repeat(digits))));
        out.push(bytes(&format!("{sign}{}e1", "7".repeat(digits))));
    }
    for depth in [511, 512, 513, 600] {
        out.push(bytes(&format!(
            "{}{}",
            "[".repeat(depth),
            "]".repeat(depth)
        )));
        out.push(bytes(&format!(
            "{}{}",
            "{\"a\":[".repeat(depth / 2),
            "]}".repeat(depth / 2)
        )));
        out.push(bytes(&format!("\"{}\"", "[{".repeat(depth))));
        out.push(bytes(&"[".repeat(depth)));
        out.push(bytes(&format!("{}\"", "[".repeat(depth))));
        out.push(bytes(&format!("[\"\\\"{}\"]", "[".repeat(depth))));
    }
    out.push(bytes(&format!("0.{}", "1".repeat(400))));
    out.push(bytes(&format!("{}.5", "1".repeat(400))));
    for digits in [
        "9".repeat(4300),
        "9".repeat(4299),
        format!("-{}", "9".repeat(4300)),
        "0".into(),
        "-1".into(),
    ] {
        out.push(JsonCase::PlusOne(digits));
    }
    out
}

const CHARS: [&str; 22] = [
    "a",
    "Z",
    " ",
    "\\\"",
    "\\\\",
    "\\/",
    "\\b",
    "\\n",
    "\\t",
    "\\u0000",
    "\\u001F",
    "é",
    "\u{2028}",
    "😀",
    "\\ud83d\\ude00",
    "\\uD800",
    "\\udfff",
    "\u{7f}",
    "\\u20ac",
    "€",
    "'",
    "\u{fffd}",
];

/// Random JSON text, sometimes damaged.
pub(super) fn random(tape: &mut Tape) -> Vec<u8> {
    let mut out = String::new();
    value(tape, &mut out, 0);
    space(tape, &mut out);
    let mut bytes = out.into_bytes();
    if tape.chance(15) && !bytes.is_empty() {
        let at = tape.below(bytes.len());
        match tape.below(5) {
            0 => bytes.truncate(at),
            1 => drop(bytes.remove(at)),
            2 => bytes.insert(at, u8::try_from(tape.below(256)).unwrap_or(0)),
            3 => bytes.insert(at, *b"[{]},:\"\\0e-.".get(tape.below(13)).unwrap_or(&b' ')),
            _ => {
                if let Some(byte) = bytes.get_mut(at) {
                    *byte = u8::try_from(tape.below(256)).unwrap_or(0);
                }
            }
        }
    }
    bytes
}

fn space(tape: &mut Tape, out: &mut String) {
    if tape.chance(25) {
        out.push_str(
            [" ", "\n", "\t", "\r\n", "  "]
                .get(tape.below(5))
                .copied()
                .unwrap_or(" "),
        );
    }
}

fn value(tape: &mut Tape, out: &mut String, depth: usize) {
    space(tape, out);
    let kinds = if depth > 6 { 4 } else { 7 };
    let kind = tape.below(kinds);
    match kind {
        0 => out.push_str(
            ["null", "true", "false"]
                .get(tape.below(3))
                .copied()
                .unwrap_or("null"),
        ),
        1 | 2 => number(tape, out),
        3 => string(tape, out),
        4 | 5 => {
            out.push('[');
            for index in 0..tape.below(4) {
                if index > 0 {
                    out.push(',');
                }
                value(tape, out, depth + 1);
            }
            space(tape, out);
            out.push(']');
        }
        _ => {
            out.push('{');
            for index in 0..tape.below(4) {
                if index > 0 {
                    out.push(',');
                }
                space(tape, out);
                if tape.chance(20) {
                    out.push_str("\"k\"");
                } else {
                    string(tape, out);
                }
                space(tape, out);
                out.push(':');
                value(tape, out, depth + 1);
            }
            space(tape, out);
            out.push('}');
        }
    }
}

fn number(tape: &mut Tape, out: &mut String) {
    if tape.chance(30) {
        out.push('-');
    }
    let longest = if tape.chance(10) { 40 } else { 6 };
    let digits = 1 + tape.below(longest);
    for index in 0..digits {
        let digit = tape.below(10);
        out.push(char::from(
            b'0' + u8::try_from(if index == 0 && digits > 1 {
                digit.max(1)
            } else {
                digit
            })
            .unwrap_or(0),
        ));
    }
    if tape.chance(40) {
        out.push('.');
        for _ in 0..1 + tape.below(18) {
            out.push(char::from(b'0' + u8::try_from(tape.below(10)).unwrap_or(0)));
        }
    }
    if tape.chance(25) {
        out.push(if tape.chance(50) { 'e' } else { 'E' });
        if tape.chance(50) {
            out.push(if tape.chance(50) { '-' } else { '+' });
        }
        out.push_str(&tape.below(330).to_string());
    }
}

fn string(tape: &mut Tape, out: &mut String) {
    out.push('"');
    for _ in 0..tape.below(8) {
        out.push_str(CHARS.get(tape.below(CHARS.len())).copied().unwrap_or("a"));
    }
    out.push('"');
}
