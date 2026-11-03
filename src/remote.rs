//! Documents on the web (`mmore https://...`), fetched with `curl`.
//!
//! GitHub pages are fetched as the files behind them: a repository
//! (`https://github.com/owner/repo`) shows its README, and a file page
//! (`.../blob/main/docs/guide.md`) the file itself, so relative links in them
//! lead to the other files of the repository.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Longest a fetch may take.
const MAX_SECONDS: &str = "30";
/// Largest document fetched (100 MB).
const MAX_BYTES: &str = "104857600";

/// Whether `arg` is a web address rather than a file name.
pub fn is_url(arg: &str) -> bool {
    let lower = arg.get(..8).unwrap_or(arg).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// What to fetch for a web address, and how to treat it.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    /// What to download.
    pub fetch: String,
    /// Ask for the raw file (GitHub's API serves a README that way).
    pub raw: bool,
    /// The address relative links are resolved against.
    pub base: String,
    /// A file name that tells what the document is (`README.md`).
    pub detect: PathBuf,
}

impl Target {
    pub fn of(url: &str) -> Target {
        if let Some(target) = github(url) {
            return target;
        }
        Target {
            fetch: url.to_string(),
            raw: false,
            base: url.to_string(),
            detect: PathBuf::from(file_name(url)),
        }
    }
}

/// GitHub repository and file pages, as the files behind them.
fn github(url: &str) -> Option<Target> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.split(['#', '?']).next().unwrap_or("");
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [owner, repo] if !owner.is_empty() && !repo.is_empty() => {
            let repo = repo.trim_end_matches(".git");
            Some(Target {
                fetch: format!("https://api.github.com/repos/{owner}/{repo}/readme"),
                raw: true,
                base: format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/"),
                detect: PathBuf::from("README.md"),
            })
        }
        [owner, repo, "blob", path @ ..] if !path.is_empty() => {
            let raw = format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/{}",
                path.join("/")
            );
            Some(Target {
                fetch: raw.clone(),
                raw: false,
                detect: PathBuf::from(path[path.len() - 1]),
                base: raw,
            })
        }
        [owner, repo, "tree", reference, dir @ ..] => {
            let dir = dir.join("/");
            let suffix = if dir.is_empty() {
                String::new()
            } else {
                format!("/{dir}")
            };
            Some(Target {
                fetch: format!(
                    "https://api.github.com/repos/{owner}/{repo}/readme{suffix}?ref={reference}"
                ),
                raw: true,
                base: format!(
                    "https://raw.githubusercontent.com/{owner}/{repo}/{reference}{suffix}/"
                ),
                detect: PathBuf::from("README.md"),
            })
        }
        _ => None,
    }
}

/// The last part of the address's path (`guide.md`), without query or fragment.
fn file_name(url: &str) -> &str {
    let path = url.split(['#', '?']).next().unwrap_or(url);
    let after_host = path.splitn(4, '/').nth(3).unwrap_or("");
    after_host.rsplit('/').next().unwrap_or("")
}

/// Download `target`. Errors are curl's message, or a note that curl is missing.
pub fn fetch(target: &Target) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("curl");
    cmd.args(["--fail", "--silent", "--show-error", "--location"])
        // Only the web: no file:// or other schemes, not even by redirection.
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", MAX_SECONDS, "--max-filesize", MAX_BYTES])
        .args(["--user-agent", concat!("mmore/", env!("CARGO_PKG_VERSION"))]);
    if target.raw {
        cmd.args(["--header", "Accept: application/vnd.github.raw"]);
    }
    let output = cmd
        .arg(&target.fetch)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("needs the curl command to fetch it ({e})"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.lines().last().unwrap_or("").trim();
    Err(if message.is_empty() {
        format!("curl failed ({})", output.status)
    } else {
        message.to_string()
    })
}

/// Resolve `link` against `base`, as a browser would: absolute links stay,
/// `/x` is from the site's root, and others are relative to the base's
/// directory, with `.` and `..` worked out.
pub fn join(base: &str, link: &str) -> String {
    if crate::render::is_absolute_url(link) {
        return link.to_string();
    }
    let scheme_end = base.find("://").map_or(0, |i| i + 3);
    let root_end = base[scheme_end..]
        .find('/')
        .map_or(base.len(), |i| scheme_end + i);
    let (root, path) = base.split_at(root_end);
    if let Some(rest) = link.strip_prefix("//") {
        return format!("{}//{rest}", &base[..scheme_end.saturating_sub(2)]);
    }
    let joined = if link.starts_with('/') {
        link.to_string()
    } else {
        let path = path.split(['?', '#']).next().unwrap_or("");
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{}{link}", if dir.is_empty() { "/" } else { dir })
    };
    // Work out `.` and `..`, leaving any query alone.
    let (path, query) = match joined.find(['?', '#']) {
        Some(i) => joined.split_at(i),
        None => (joined.as_str(), ""),
    };
    let mut parts: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    parts.push("");
                }
            }
            ".." => {
                parts.pop();
                if last {
                    parts.push("");
                }
            }
            s => parts.push(s),
        }
    }
    format!("{root}/{}{query}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_told_apart_from_files() {
        assert!(is_url("https://example.com/a.md"));
        assert!(is_url("HTTP://x.io"));
        assert!(!is_url("notes.md"));
        assert!(!is_url("https:notes"));
        assert!(!is_url("ftp://x.io/a"));
    }

    #[test]
    fn plain_addresses_are_fetched_as_they_are() {
        let t = Target::of("https://example.com/docs/guide.md?x=1#part");
        assert_eq!(t.fetch, "https://example.com/docs/guide.md?x=1#part");
        assert_eq!(t.detect, PathBuf::from("guide.md"));
        assert!(!t.raw);
        assert_eq!(Target::of("https://example.com").detect, PathBuf::from(""));
    }

    #[test]
    fn github_pages_become_their_files() {
        let repo = Target::of("https://github.com/houseofmackee/mmore/");
        assert_eq!(
            repo.fetch,
            "https://api.github.com/repos/houseofmackee/mmore/readme"
        );
        assert!(repo.raw);
        assert_eq!(
            repo.base,
            "https://raw.githubusercontent.com/houseofmackee/mmore/HEAD/"
        );
        assert_eq!(
            Target::of("https://github.com/o/r.git").fetch,
            "https://api.github.com/repos/o/r/readme"
        );
        let file = Target::of("https://github.com/o/r/blob/main/docs/guide.md");
        assert_eq!(
            file.fetch,
            "https://raw.githubusercontent.com/o/r/main/docs/guide.md"
        );
        assert_eq!(file.detect, PathBuf::from("guide.md"));
        let tree = Target::of("https://github.com/o/r/tree/v1/docs");
        assert_eq!(
            tree.fetch,
            "https://api.github.com/repos/o/r/readme/docs?ref=v1"
        );
        assert_eq!(tree.base, "https://raw.githubusercontent.com/o/r/v1/docs/");
        // Other GitHub pages are fetched as they are.
        assert_eq!(
            Target::of("https://github.com/o/r/issues/1").fetch,
            "https://github.com/o/r/issues/1"
        );
    }

    #[test]
    fn links_are_resolved_like_a_browser() {
        let base = "https://h.io/docs/guide.md";
        assert_eq!(join(base, "intro.md"), "https://h.io/docs/intro.md");
        assert_eq!(join(base, "./a/b.md"), "https://h.io/docs/a/b.md");
        assert_eq!(join(base, "../README.md"), "https://h.io/README.md");
        assert_eq!(join(base, "../../../x.md"), "https://h.io/x.md");
        assert_eq!(join(base, "/top.md"), "https://h.io/top.md");
        assert_eq!(join(base, "//cdn.io/x.md"), "https://cdn.io/x.md");
        assert_eq!(join(base, "https://o.io/y"), "https://o.io/y");
        assert_eq!(join(base, "x.md?raw=1"), "https://h.io/docs/x.md?raw=1");
        assert_eq!(join("https://h.io", "a.md"), "https://h.io/a.md");
        assert_eq!(join("https://h.io/d/", "a.md"), "https://h.io/d/a.md");
        assert_eq!(
            join("https://h.io/d/x.md?q=1", "a.md"),
            "https://h.io/d/a.md"
        );
    }
}
