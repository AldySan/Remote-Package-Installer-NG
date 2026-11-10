//! Source hygiene: no em dashes in string literals (antislop R-02).
//!
//! Round 1 of the UI audit swept the app crate only, and the core crate's
//! log/error strings (rendered verbatim in the Activity panel and status
//! labels) silently kept theirs. This test tokenizes every workspace
//! source file and fails on any em dash inside a string, char or raw
//! string literal, so the contract holds for both crates from now on.
//! Em dashes in comments are fine: comments never reach the user.

use std::path::{Path, PathBuf};

#[test]
fn no_em_dash_in_string_literals() {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut roots: Vec<PathBuf> = vec![app.join("src"), app.join("build.rs"), app.join("tests")];
    let core = app.join("../rpi-ng-core");
    roots.extend([core.join("src"), core.join("tests"), core.join("examples")]);

    let mut files: Vec<PathBuf> = Vec::new();
    for root in &roots {
        collect_rs_files(root, &mut files);
    }
    assert!(
        !files.is_empty(),
        "scanner found no sources; the layout changed"
    );

    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        // This test's own fixtures deliberately contain em dashes.
        if file.file_name().is_some_and(|n| n == "source_hygiene.rs") {
            continue;
        }
        let text = std::fs::read_to_string(file).expect("readable source file");
        for line in em_dash_lines(&text) {
            offenders.push(format!("{}:{}", file.display(), line));
        }
    }
    assert!(
        offenders.is_empty(),
        "em dash in user-facing string literals (use a period, colon, \
         semicolon or parentheses instead):\n  {}",
        offenders.join("\n  ")
    );
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Line number of every em dash that sits inside a string/char/raw-string
/// literal. A tiny Rust tokenizer: enough states to tell literals from
/// comments, handling escapes, string line-continuations, nested block
/// comments, raw strings (`r"…"`, `r#"…"#`) and lifetimes.
fn em_dash_lines(text: &str) -> Vec<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        Str,
        RawStr(usize), // number of leading `#`
        LineComment,
        BlockComment(usize),
    }

    let mut hits = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut state = State::Code;
    let mut line = 1usize;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();

        if c == '\n' {
            line += 1;
            i += 1;
            if state == State::LineComment {
                state = State::Code;
            }
            continue;
        }

        match state {
            State::Code => match c {
                '/' if next == Some('/') => {
                    state = State::LineComment;
                    i += 2;
                }
                '/' if next == Some('*') => {
                    state = State::BlockComment(1);
                    i += 2;
                }
                '"' => {
                    state = State::Str;
                    i += 1;
                }
                'r' if next == Some('"') => {
                    state = State::RawStr(0);
                    i += 2;
                }
                'r' if next == Some('#') => {
                    let mut hashes = 0usize;
                    let mut j = i + 1;
                    while chars.get(j) == Some(&'#') {
                        hashes += 1;
                        j += 1;
                    }
                    if chars.get(j) == Some(&'"') {
                        state = State::RawStr(hashes);
                        i = j + 1;
                    } else {
                        // Plain identifier starting with 'r'.
                        i += 1;
                    }
                }
                '\'' => {
                    // Char literal ('x', '\n', '\u{2713}') vs lifetime ('a).
                    let is_char = match next {
                        Some('\\') => true,
                        Some(q) if q != '\'' => chars.get(i + 2) == Some(&'\''),
                        _ => false,
                    };
                    if is_char {
                        let mut j = i + 1;
                        while j < chars.len() && chars[j] != '\'' {
                            j += if chars[j] == '\\' { 2 } else { 1 };
                        }
                        i = (j + 1).min(chars.len());
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            },
            State::Str => match c {
                '\\' => i += 2,
                '"' => {
                    state = State::Code;
                    i += 1;
                }
                '—' => {
                    hits.push(line);
                    i += 1;
                }
                _ => i += 1,
            },
            State::RawStr(hashes) => {
                if c == '"' {
                    let mut j = i + 1;
                    let mut seen = 0usize;
                    while seen < hashes && chars.get(j) == Some(&'#') {
                        seen += 1;
                        j += 1;
                    }
                    if seen == hashes {
                        state = State::Code;
                        i = j;
                    } else {
                        i += 1;
                    }
                } else {
                    if c == '—' {
                        hits.push(line);
                    }
                    i += 1;
                }
            }
            State::LineComment => i += 1,
            State::BlockComment(depth) => {
                if c == '/' && next == Some('*') {
                    state = State::BlockComment(depth + 1);
                    i += 2;
                } else if c == '*' && next == Some('/') {
                    state = if depth == 1 {
                        State::Code
                    } else {
                        State::BlockComment(depth - 1)
                    };
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::em_dash_lines;

    #[test]
    fn tokenizer_sees_strings_but_not_comments() {
        let src = "// comment — fine\nlet s = \"has — one\"; /* block — fine */\n";
        assert_eq!(em_dash_lines(src), vec![2]);
    }

    #[test]
    fn tokenizer_handles_raw_strings_and_lifetimes() {
        let src = "let a: &'a str = r#\"raw — dash\"#;\nlet b = 'x';\n/// doc — fine\nfn f<'b>(x: &'b str) {}\n";
        assert_eq!(em_dash_lines(src), vec![1]);
    }

    #[test]
    fn tokenizer_handles_escapes_and_continuations() {
        let src = "let s = \"escaped \\\" quote — dash\";\nlet t = \"multi \\\nline — dash\";\n";
        assert_eq!(em_dash_lines(src), vec![1, 2]);
    }
}
