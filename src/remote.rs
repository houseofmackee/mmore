//! Documents on the web (`mmore https://...`), fetched with `curl`.
//!
//! GitHub, GitLab (gitlab.com), and Codeberg pages are fetched as the files
//! behind them: a repository (`https://github.com/owner/repo`) or folder shows
//! its README, and a file page (`.../blob/main/docs/guide.md`) the file itself,
//! so relative links in them lead to the other files of the repository.

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
    /// The README's name must be looked up first (GitLab, Codeberg).
    pub lookup: Option<Lookup>,
}

/// How to find out which file a folder's README is.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// GitLab's project information, which has a `readme_url`.
    GitLabProject { api: String },
    /// A list of the folder's files (JSON with `name`s), and where the raw
    /// files of that folder are.
    Files { api: String, raw_dir: String },
}

impl Target {
    pub fn of(url: &str) -> Target {
        if let Some(target) = github(url)
            .or_else(|| gitlab(url))
            .or_else(|| codeberg(url))
        {
            return target;
        }
        Target::plain(url.to_string())
    }

    /// The address itself, judged by its file name.
    fn plain(url: String) -> Target {
        Target {
            detect: PathBuf::from(file_name(&url)),
            base: url.clone(),
            fetch: url,
            raw: false,
            lookup: None,
        }
    }

    /// A folder's README, whose name is looked up first.
    fn readme(lookup: Lookup, base: String) -> Target {
        Target {
            fetch: String::new(),
            raw: false,
            base,
            detect: PathBuf::from("README.md"),
            lookup: Some(lookup),
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
                lookup: None,
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
                lookup: None,
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
                lookup: None,
            })
        }
        _ => None,
    }
}

/// The parts of the path after `prefix` (a site), without query, fragment, or
/// trailing slash.
fn site_path<'a>(url: &'a str, host: &str) -> Option<Vec<&'a str>> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?
        .strip_prefix(host)?
        .strip_prefix('/')?;
    let rest = rest.split(['#', '?']).next().unwrap_or("");
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    (parts.len() >= 2 && parts.iter().all(|p| !p.is_empty())).then_some(parts)
}

/// GitLab (gitlab.com) projects, which can be in nested groups: the project's
/// README, a file (`/-/blob/REF/path`), or a folder's README (`/-/tree/REF/dir`).
fn gitlab(url: &str) -> Option<Target> {
    let parts = site_path(url, "gitlab.com")?;
    let dash = parts.iter().position(|p| *p == "-");
    let project = parts[..dash.unwrap_or(parts.len())].join("/");
    let project = project.trim_end_matches(".git").to_string();
    let api = format!(
        "https://gitlab.com/api/v4/projects/{}",
        project.replace('/', "%2F")
    );
    let Some(dash) = dash else {
        let base = format!("https://gitlab.com/{project}/-/raw/HEAD/");
        return Some(Target::readme(Lookup::GitLabProject { api }, base));
    };
    match &parts[dash + 1..] {
        ["blob", reference, path @ ..] if !path.is_empty() => Some(Target::plain(format!(
            "https://gitlab.com/{project}/-/raw/{reference}/{}",
            path.join("/")
        ))),
        ["tree", reference, dir @ ..] => {
            let dir = dir.join("/");
            let raw_dir = format!(
                "https://gitlab.com/{project}/-/raw/{reference}/{}",
                if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                }
            );
            let api = format!("{api}/repository/tree?path={dir}&ref={reference}");
            Some(Target::readme(
                Lookup::Files {
                    api,
                    raw_dir: raw_dir.clone(),
                },
                raw_dir,
            ))
        }
        _ => None,
    }
}

/// Codeberg (Forgejo) repositories: the README, a file
/// (`/src/branch/REF/path`, also `tag` and `commit`), or a folder's README.
/// Whether a `/src/` path is a file or a folder is judged by a dot in its name.
fn codeberg(url: &str) -> Option<Target> {
    let parts = site_path(url, "codeberg.org")?;
    let (owner, repo) = (parts[0], parts[1].trim_end_matches(".git"));
    let api = format!("https://codeberg.org/api/v1/repos/{owner}/{repo}/contents");
    let site = format!("https://codeberg.org/{owner}/{repo}");
    match &parts[2..] {
        [] => {
            let raw_dir = format!("{site}/raw/HEAD/");
            Some(Target::readme(
                Lookup::Files {
                    api,
                    raw_dir: raw_dir.clone(),
                },
                raw_dir,
            ))
        }
        [
            "src",
            kind @ ("branch" | "tag" | "commit"),
            reference,
            path @ ..,
        ] => {
            let path = path.join("/");
            let is_file = path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.contains('.'));
            if is_file {
                return Some(Target::plain(format!(
                    "{site}/raw/{kind}/{reference}/{path}"
                )));
            }
            let raw_dir = format!(
                "{site}/raw/{kind}/{reference}/{}",
                if path.is_empty() {
                    String::new()
                } else {
                    format!("{path}/")
                }
            );
            let api = if path.is_empty() {
                format!("{api}?ref={reference}")
            } else {
                format!("{api}/{path}?ref={reference}")
            };
            Some(Target::readme(
                Lookup::Files {
                    api,
                    raw_dir: raw_dir.clone(),
                },
                raw_dir,
            ))
        }
        _ => None,
    }
}

/// The string values of `key` in JSON text, in order. Enough for the simple,
/// flat answers of the GitLab and Forgejo APIs.
fn json_strings(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut rest = json;
    while let Some(i) = rest.find(&needle) {
        rest = rest[i + needle.len()..].trim_start();
        let Some(after) = rest.strip_prefix(':').map(str::trim_start) else {
            continue;
        };
        let Some(body) = after.strip_prefix('"') else {
            rest = after;
            continue;
        };
        let mut value = String::new();
        let mut chars = body.char_indices();
        let mut end = body.len();
        while let Some((at, c)) = chars.next() {
            match c {
                '"' => {
                    end = at + 1;
                    break;
                }
                '\\' => match chars.next() {
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, 't')) => value.push('\t'),
                    Some((_, other)) => value.push(other),
                    None => {}
                },
                c => value.push(c),
            }
        }
        values.push(value);
        rest = &body[end.min(body.len())..];
    }
    values
}

/// Which of a folder's files is its README: `README.md` if there is one,
/// then another Markdown README, then any README.
fn pick_readme(names: &[String]) -> Option<&str> {
    let lower = |n: &String| n.to_ascii_lowercase();
    names
        .iter()
        .find(|n| lower(n) == "readme.md")
        .or_else(|| {
            names.iter().find(|n| {
                let n = lower(n);
                n.starts_with("readme.") && (n.ends_with(".md") || n.ends_with(".markdown"))
            })
        })
        .or_else(|| names.iter().find(|n| lower(n).starts_with("readme")))
        .map(String::as_str)
}

/// Find the README's name (for a `lookup`), so `target` points at the file.
fn look_up(target: &mut Target) -> Result<(), String> {
    let Some(lookup) = target.lookup.take() else {
        return Ok(());
    };
    match lookup {
        Lookup::GitLabProject { api } => {
            let json = String::from_utf8_lossy(&download(&api, false)?).into_owned();
            let readme = json_strings(&json, "readme_url")
                .into_iter()
                .next()
                .filter(|u| !u.is_empty())
                .ok_or("the project has no README")?;
            let raw = readme.replacen("/-/blob/", "/-/raw/", 1);
            target.detect = PathBuf::from(file_name(&raw));
            target.base = raw.clone();
            target.fetch = raw;
        }
        Lookup::Files { api, raw_dir } => {
            let json = String::from_utf8_lossy(&download(&api, false)?).into_owned();
            let names = json_strings(&json, "name");
            let name = pick_readme(&names).ok_or("there is no README here")?;
            target.detect = PathBuf::from(name);
            target.fetch = format!("{raw_dir}{name}");
            target.base = raw_dir;
        }
    }
    Ok(())
}

/// The last part of the address's path (`guide.md`), without query or fragment.
fn file_name(url: &str) -> &str {
    let path = url.split(['#', '?']).next().unwrap_or(url);
    let after_host = path.splitn(4, '/').nth(3).unwrap_or("");
    after_host.rsplit('/').next().unwrap_or("")
}

/// Download `target` (looking up a README's name first, if needed; `target` is
/// updated to point at the file). Errors are curl's message, or a note that
/// curl is missing.
pub fn fetch(target: &mut Target) -> Result<Vec<u8>, String> {
    look_up(target)?;
    download(&target.fetch, target.raw)
}

fn download(url: &str, raw: bool) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("curl");
    cmd.args(["--fail", "--silent", "--show-error", "--location"])
        // Only the web: no file:// or other schemes, not even by redirection.
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", MAX_SECONDS, "--max-filesize", MAX_BYTES])
        .args(["--user-agent", concat!("mmore/", env!("CARGO_PKG_VERSION"))]);
    if raw {
        cmd.args(["--header", "Accept: application/vnd.github.raw"]);
    }
    let output = cmd
        .arg(url)
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
    fn gitlab_and_codeberg_pages_become_their_files() {
        let repo = Target::of("https://gitlab.com/group/sub/project");
        assert_eq!(
            repo.lookup,
            Some(Lookup::GitLabProject {
                api: "https://gitlab.com/api/v4/projects/group%2Fsub%2Fproject".into()
            })
        );
        let file = Target::of("https://gitlab.com/group/project/-/blob/main/docs/a.md");
        assert_eq!(
            file.fetch,
            "https://gitlab.com/group/project/-/raw/main/docs/a.md"
        );
        assert_eq!(file.detect, PathBuf::from("a.md"));
        assert_eq!(file.lookup, None);
        let tree = Target::of("https://gitlab.com/g/p/-/tree/v2/docs");
        assert_eq!(
            tree.lookup,
            Some(Lookup::Files {
                api: "https://gitlab.com/api/v4/projects/g%2Fp/repository/tree?path=docs&ref=v2"
                    .into(),
                raw_dir: "https://gitlab.com/g/p/-/raw/v2/docs/".into()
            })
        );
        let repo = Target::of("https://codeberg.org/o/r/");
        assert_eq!(
            repo.lookup,
            Some(Lookup::Files {
                api: "https://codeberg.org/api/v1/repos/o/r/contents".into(),
                raw_dir: "https://codeberg.org/o/r/raw/HEAD/".into()
            })
        );
        let file = Target::of("https://codeberg.org/o/r/src/tag/v1/docs/guide.md");
        assert_eq!(
            file.fetch,
            "https://codeberg.org/o/r/raw/tag/v1/docs/guide.md"
        );
        let dir = Target::of("https://codeberg.org/o/r/src/branch/main/docs");
        assert_eq!(
            dir.lookup,
            Some(Lookup::Files {
                api: "https://codeberg.org/api/v1/repos/o/r/contents/docs?ref=main".into(),
                raw_dir: "https://codeberg.org/o/r/raw/branch/main/docs/".into()
            })
        );
        // Other pages of those sites are fetched as they are.
        assert_eq!(
            Target::of("https://codeberg.org/o/r/issues").fetch,
            "https://codeberg.org/o/r/issues"
        );
        assert_eq!(
            Target::of("https://gitlab.com/explore").fetch,
            "https://gitlab.com/explore"
        );
    }

    #[test]
    fn readmes_are_found_in_file_lists() {
        let json = r#"[{"name":"LICENSE","type":"file"},{"name": "readme.rst"},{"name":"README.md","path":"x \"y\""}]"#;
        let names = json_strings(json, "name");
        assert_eq!(names, ["LICENSE", "readme.rst", "README.md"]);
        assert_eq!(pick_readme(&names), Some("README.md"));
        assert_eq!(pick_readme(&names[..2]), Some("readme.rst"));
        assert_eq!(pick_readme(&names[..1]), None);
        let project =
            r#"{"id":1,"readme_url":"https://gitlab.com/g/p/-/blob/main/README.md","x":null}"#;
        assert_eq!(
            json_strings(project, "readme_url"),
            ["https://gitlab.com/g/p/-/blob/main/README.md"]
        );
        assert!(json_strings(r#"{"readme_url":null}"#, "readme_url").is_empty());
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
