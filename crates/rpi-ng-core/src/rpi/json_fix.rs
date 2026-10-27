//! Tolerant JSON preprocessing for the RPI dialect.
//!
//! The upstream payload writes unquoted hexadecimal literals
//! (`{"bits":0x20,...}`), which is invalid JSON (`docs/BASELINE.md` §3.1).
//! [`repair`] rewrites such literals to decimal integers while staying aware of
//! JSON strings (a `": 0x…"` *inside* a quoted value must not be touched).

/// Rewrite unquoted `0x…` number literals into plain decimal.
///
/// String-aware: scans the document tracking whether we are inside a quoted
/// string (handling `\` escapes), and only rewrites number tokens that appear
/// in value position (after `:` or `,` or `[`), never inside strings.
pub fn repair(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len() + 16);
    let mut i = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            out.push(b as char);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                in_string = true;
                out.push('"');
                i += 1;
            }
            b':' | b',' | b'[' => {
                out.push(b as char);
                i += 1;
                // skip whitespace
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r' || bytes[i] == b'\n') {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                // hex literal?
                if i + 1 < bytes.len() && bytes[i] == b'0' && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X') {
                    let mut j = i + 2;
                    while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                        j += 1;
                    }
                    if j > i + 2 {
                        let hex = &body[i + 2..j];
                        match u64::from_str_radix(hex, 16) {
                            Ok(v) => out.push_str(&v.to_string()),
                            Err(_) => out.push_str(&body[i..j]), // unreachable for hexdigits
                        }
                        i = j;
                    }
                }
            }
            _ => {
                // copy ASCII byte; non-ASCII must pass through as part of a
                // UTF-8 sequence — handled since we only special-case ASCII above.
                if b < 0x80 {
                    out.push(b as char);
                    i += 1;
                } else {
                    // Find the full UTF-8 sequence start; copy char by char.
                    let rest = &body[i..];
                    let ch = rest.chars().next().unwrap_or('\u{FFFD}');
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::repair;

    #[test]
    fn repairs_unquoted_hex() {
        let raw = r#"{"status":"success","bits": 0x20, "error":0, "length": 0x1000, "transferred":0x0, "num_index":0, "num_total":3}"#;
        let fixed = repair(raw);
        let v: serde_json::Value = serde_json::from_str(&fixed).expect("valid json");
        assert_eq!(v["bits"], 32);
        assert_eq!(v["length"], 4096);
        assert_eq!(v["num_total"], 3);
    }

    #[test]
    fn repairs_error_code_hex() {
        let raw = r#"{ "status": "fail", "error_code": 0x80990005 }"#;
        let v: serde_json::Value = serde_json::from_str(&repair(raw)).unwrap();
        assert_eq!(v["error_code"], 0x80990005u64 as u64);
    }

    #[test]
    fn leaves_strings_alone() {
        let raw = r#"{"status":"success","title":"Weird: 0xdead value"}"#;
        let v: serde_json::Value = serde_json::from_str(&repair(raw)).unwrap();
        assert_eq!(v["title"], "Weird: 0xdead value");
    }

    #[test]
    fn handles_escaped_strings() {
        let raw = r#"{"title":"quote \" then : 0x1 still string"}"#;
        let v: serde_json::Value = serde_json::from_str(&repair(raw)).unwrap();
        assert_eq!(v["title"], r#"quote " then : 0x1 still string"#);
    }

    #[test]
    fn passes_valid_json_through_unchanged_semantically() {
        let raw = r#"{"status":"success","task_id":7,"title":"Game"}"#;
        let v: serde_json::Value = serde_json::from_str(&repair(raw)).unwrap();
        assert_eq!(v["task_id"], 7);
    }

    #[test]
    fn uppercase_hex_and_arrays() {
        let raw = r#"{"bits": 0XAB, "pieces":[0x1, 0x2, 0x10]}"#;
        let v: serde_json::Value = serde_json::from_str(&repair(raw)).unwrap();
        assert_eq!(v["bits"], 0xAB);
        assert_eq!(v["pieces"][2], 16);
    }
}
