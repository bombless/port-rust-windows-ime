//! Dev tool: reads one composition per line from stdin and prints the engine's
//! candidates as JSON lines, in the same shape as the Go reference tool used for
//! differential testing (`text|annotation` per candidate).
use std::io::{BufRead, Write};

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn main() {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let items: Vec<String> = port_rust_windows_ime::engine::candidates_for(&line)
            .iter()
            .map(|c| format!("\"{}|{}\"", json_escape(&c.text), json_escape(&c.annotation)))
            .collect();
        writeln!(out, "[{}]", items.join(",")).unwrap();
    }
}
