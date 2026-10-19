//! Bounded file search and grep executed on the Target in one request.
use crate::protocol;
use anyhow::{ensure, Context, Result};
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

// Owner-session filesystem calls are cut off after 30 seconds.
const BUDGET: Duration = Duration::from_secs(20);
const MAX_ENTRIES: usize = 200_000;
const GREP_FILE_LIMIT: u64 = 4 * 1024 * 1024;
const LINE_LIMIT: usize = 400;
const SKIPPED: [&str; 8] = [
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
];

pub fn search(root: &Path, p: &Value) -> Result<Value> {
    let patterns = globs(protocol::string(p, "pattern")?);
    let max = p["maxResults"].as_u64().unwrap_or(100).clamp(1, 1000) as usize;
    let mut matches = Vec::new();
    let mut stopped = walk(root, p["includeIgnored"] == true, &mut |path, relative| {
        if patterns.iter().any(|g| glob_match(g, relative)) {
            matches.push(path.to_string_lossy().into_owned());
        }
        matches.len() <= max
    })?;
    if matches.len() > max {
        matches.truncate(max);
        stopped = Some("maxResults");
    }
    Ok(json!({"root":root.to_string_lossy(),"matches":matches,"stoppedBy":stopped}))
}

pub fn grep(root: &Path, p: &Value) -> Result<Value> {
    let regex = RegexBuilder::new(protocol::string(p, "pattern")?)
        .case_insensitive(p["ignoreCase"] == true)
        .size_limit(1 << 20)
        .build()
        .context("E_ARGUMENT: invalid regex pattern")?;
    let filter = p["glob"]
        .as_str()
        .filter(|g| !g.is_empty())
        .map(globs);
    let max = p["maxMatches"].as_u64().unwrap_or(50).clamp(1, 500) as usize;
    let mut matches = Vec::new();
    let mut scanned = 0u64;
    let mut stopped = walk(root, p["includeIgnored"] == true, &mut |path, relative| {
        if filter
            .as_ref()
            .is_some_and(|g| !g.iter().any(|g| glob_match(g, relative)))
        {
            return true;
        }
        let Some(bytes) = small_file(path) else {
            return true;
        };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return true;
        }
        scanned += 1;
        for (index, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if regex.is_match(line) {
                matches.push(json!({"path":path.to_string_lossy(),"line":index + 1,"text":clip(line)}));
                if matches.len() > max {
                    return false;
                }
            }
        }
        true
    })?;
    if matches.len() > max {
        matches.truncate(max);
        stopped = Some("maxResults");
    }
    Ok(json!({"root":root.to_string_lossy(),"matches":matches,"filesScanned":scanned,"stoppedBy":stopped}))
}

/// Visits regular files in sorted depth-first order. Symlinks are never followed.
/// Returns why the walk stopped early, if it did.
fn walk(
    root: &Path,
    include_ignored: bool,
    visit: &mut dyn FnMut(&Path, &str) -> bool,
) -> Result<Option<&'static str>> {
    let started = Instant::now();
    let metadata = fs::metadata(root).with_context(|| format!("E_NOT_FOUND: {}", root.display()))?;
    if metadata.is_file() {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Ok((!visit(root, &name)).then_some("maxResults"));
    }
    ensure!(metadata.is_dir(), "E_ARGUMENT: path must be a file or directory");
    let mut pending = vec![(root.to_path_buf(), String::new())];
    let mut visited = 0usize;
    while let Some((directory, prefix)) = pending.pop() {
        let Ok(reader) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<_> = reader.filter_map(|entry| entry.ok()).collect();
        entries.sort_by_key(|entry| entry.file_name());
        let mut children = Vec::new();
        for entry in entries {
            visited += 1;
            if visited > MAX_ENTRIES {
                return Ok(Some("entryLimit"));
            }
            if started.elapsed() > BUDGET {
                return Ok(Some("timeLimit"));
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_dir() {
                if include_ignored || !SKIPPED.contains(&name.as_str()) {
                    children.push((entry.path(), relative));
                }
            } else if kind.is_file() && !visit(&entry.path(), &relative) {
                return Ok(Some("maxResults"));
            }
        }
        pending.extend(children.into_iter().rev());
    }
    Ok(None)
}
fn small_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > GREP_FILE_LIMIT {
        return None;
    }
    fs::read(path).ok()
}
fn clip(line: &str) -> String {
    let line = line.trim_end_matches('\r');
    match line.char_indices().nth(LINE_LIMIT) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line.to_owned(),
    }
}

/// Expands one level of {a,b} alternatives per brace group.
fn globs(pattern: &str) -> Vec<String> {
    let pattern = pattern.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./");
    let (Some(open), Some(close)) = (pattern.find('{'), pattern.find('}')) else {
        return vec![pattern.to_owned()];
    };
    if close < open {
        return vec![pattern.to_owned()];
    }
    pattern[open + 1..close]
        .split(',')
        .flat_map(|choice| {
            globs(&format!(
                "{}{}{}",
                &pattern[..open],
                choice,
                &pattern[close + 1..]
            ))
        })
        .collect()
}
/// A pattern without / matches the file name at any depth; otherwise the whole
/// relative path. * and ? stay within one path segment, ** crosses segments.
fn glob_match(pattern: &str, relative: &str) -> bool {
    let target = if pattern.contains('/') {
        relative
    } else {
        relative.rsplit('/').next().unwrap_or(relative)
    };
    let fold = |s: &str| -> Vec<char> {
        if cfg!(windows) {
            s.to_lowercase().chars().collect()
        } else {
            s.chars().collect()
        }
    };
    matches(&fold(pattern), &fold(target))
}
fn matches(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') if p.get(1) == Some(&'*') => {
            if p.get(2) == Some(&'/') {
                let rest = &p[3..];
                matches(rest, t)
                    || t.iter()
                        .enumerate()
                        .any(|(i, c)| *c == '/' && matches(rest, &t[i + 1..]))
            } else {
                let rest = &p[2..];
                (0..=t.len()).any(|i| matches(rest, &t[i..]))
            }
        }
        Some('*') => {
            let rest = &p[1..];
            let mut i = 0;
            loop {
                if matches(rest, &t[i..]) {
                    return true;
                }
                if i == t.len() || t[i] == '/' {
                    return false;
                }
                i += 1;
            }
        }
        Some('?') => t.first().is_some_and(|c| *c != '/') && matches(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && matches(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_follow_segment_rules() {
        assert!(glob_match("*.rs", "src/core/lib.rs"));
        assert!(glob_match("src/**/*.rs", "src/core/lib.rs"));
        assert!(glob_match("src/**/*.rs", "src/lib.rs"));
        assert!(!glob_match("src/*.rs", "src/core/lib.rs"));
        assert!(!glob_match("**/x", "dir/ax"));
        assert!(glob_match("Cargo.tom?", "crates/cli/Cargo.toml"));
        let both = globs("src/*.{ts,tsx}");
        assert!(both.iter().any(|g| glob_match(g, "src/app.tsx")));
        assert!(both.iter().any(|g| glob_match(g, "src/app.ts")));
        assert!(!both.iter().any(|g| glob_match(g, "src/app.js")));
    }
    #[test]
    fn search_skips_dependency_folders_and_caps_results() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["a.rs", "src/b.rs", "node_modules/c.rs", "target/d.rs", "src/e.txt"] {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let found = search(dir.path(), &json!({"pattern":"*.rs"})).unwrap();
        let names: Vec<_> = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| Path::new(m.as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.rs", "b.rs"]);
        assert!(found["stoppedBy"].is_null());
        let all = search(dir.path(), &json!({"pattern":"*.rs","includeIgnored":true})).unwrap();
        assert_eq!(all["matches"].as_array().unwrap().len(), 4);
        let capped = search(dir.path(), &json!({"pattern":"*.rs","includeIgnored":true,"maxResults":3})).unwrap();
        assert_eq!(capped["matches"].as_array().unwrap().len(), 3);
        assert_eq!(capped["stoppedBy"], "maxResults");
    }
    #[test]
    fn grep_returns_line_numbers_and_skips_binary() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {\r\n    run();\r\n}\r\n").unwrap();
        fs::write(dir.path().join("notes.md"), "run later\n").unwrap();
        fs::write(dir.path().join("blob.bin"), [b'r', b'u', b'n', 0]).unwrap();
        let found = grep(dir.path(), &json!({"pattern":r"\brun\("})).unwrap();
        let matches = found["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["line"], 2);
        assert_eq!(matches[0]["text"], "    run();");
        assert_eq!(found["filesScanned"], 2);
        let filtered = grep(dir.path(), &json!({"pattern":"RUN","ignoreCase":true,"glob":"*.md"})).unwrap();
        assert_eq!(filtered["matches"].as_array().unwrap().len(), 1);
        assert!(grep(dir.path(), &json!({"pattern":"("})).unwrap_err().to_string().contains("E_ARGUMENT"));
    }
}
