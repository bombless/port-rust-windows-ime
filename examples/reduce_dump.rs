//! Dev tool: reads one key sequence per line (space separated hex virtual-key
//! codes), feeds it to a fresh session and prints every step as JSON, in the same
//! shape as the Go reference harness: `[consume, composition, selected, commit|null]`.
use std::io::{BufRead, Write};

use port_rust_windows_ime::session::Session;

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn main() {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in std::io::stdin().lock().lines() {
        let session = Session::headless();
        let mut steps = Vec::new();
        for field in line.unwrap().split_whitespace() {
            let vk = usize::from_str_radix(field, 16).expect("hex virtual-key code");
            // A key with no mapping belongs to the application: no reply, no state change.
            let (consume, commit) = match session.apply_key(vk) {
                Some(reply) => (reply.consume, reply.commit),
                None => (false, None),
            };
            let state = session.ime_state();
            steps.push(format!(
                "[{consume},{},{},{}]",
                json_string(&state.composition),
                state.selected,
                commit.map_or("null".to_owned(), |c| json_string(&c))
            ));
        }
        writeln!(out, "[{}]", steps.join(",")).unwrap();
    }
}
