//! Bans, by name, every f32 method Rust 1.95's docs call non-deterministic
//! (std/primitive.f32.html, extracted 2026-11-10: 33 maths functions with
//! "unspecified precision", plus max/min/clamp, which pick either zero when
//! given +0.0 and -0.0). Also bans HashMap/HashSet (iteration order is
//! randomized per process). `src/det.rs` is the one allowed home of the
//! deterministic replacements, and it calls libm, never these.

use std::path::Path;

const BANNED_METHODS: [&str; 36] = [
    "abs_sub", "acos", "acosh", "asin", "asinh", "atan", "atan2", "atanh", "cbrt", "clamp", "cos",
    "cosh", "erf", "erfc", "exp", "exp2", "exp_m1", "gamma", "hypot", "ln", "ln_1p", "ln_gamma",
    "log", "log10", "log2", "max", "min", "powf", "powi", "sin", "sin_cos", "sinh", "tan", "tanh",
    "to_degrees", "to_radians",
];
const BANNED_WORDS: [&str; 2] = ["HashMap", "HashSet"];

/// Findings as "file:line: what". Method calls are matched as `.name(`, so
/// `det::max(` and `libm::sinf(` pass while `x.max(` and `a.sin(` do not.
fn scan(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for m in BANNED_METHODS {
                    if code.contains(&format!(".{m}(")) {
                        out.push(format!("{}:{}: .{m}(", p.display(), n + 1));
                    }
                }
                for w in BANNED_WORDS {
                    if code.contains(w) {
                        out.push(format!("{}:{}: {w}", p.display(), n + 1));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn sim_source_uses_no_nondeterministic_float_methods() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = scan(&src);
    assert!(found.is_empty(), "use crate::det instead:\n{}", found.join("\n"));
}

/// The scanner must actually fire: plant each kind of violation in a scratch
/// directory and expect it to be reported.
#[test]
fn scanner_catches_planted_violations() {
    let dir = std::env::temp_dir().join(format!("lifeapi-hygiene-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.rs"),
        "fn f(a: f32) -> f32 { a.sin() + a.max(0.0) }\nuse std::collections::HashMap;\n\
         fn ok(a: f32) -> f32 { det::max(a, 0.0) } // a.cos( in a comment is fine\n",
    )
    .unwrap();
    let found = scan(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(found.len(), 3, "{found:#?}");
}
