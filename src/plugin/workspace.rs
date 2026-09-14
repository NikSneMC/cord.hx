//! Workspace detection: VCS roots, git repositories and worktrees.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::util;

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub dir: String,
    /// Repository name for git repositories (also for linked worktrees), or
    /// the directory name.
    pub name: String,
    /// Directory name of a linked git worktree.
    pub worktree: Option<String>,
    pub repo_url: Option<String>,
}

pub struct Search<'a> {
    pub root_markers: &'a [String],
    /// Stop at this directory (`advanced.workspace.limit-to-cwd?`).
    pub limit: Option<&'a str>,
    pub cwd: &'a str,
}

/// Walks up from `initial` looking for a root marker. Returns `initial` if
/// the filesystem root is reached.
pub fn find_vcs_root(initial: &str, search: &Search<'_>) -> String {
    let mut dir = initial.to_string();
    loop {
        if search
            .root_markers
            .iter()
            .any(|m| Path::new(&format!("{dir}/{m}")).exists())
        {
            return dir;
        }
        if search.limit == Some(dir.as_str()) {
            return dir;
        }
        let parent = util::head(&dir);
        if parent == dir {
            return initial.to_string();
        }
        dir = parent;
    }
}

/// Finds the workspace directory of a buffer path (empty for scratch buffers).
pub fn find(path: &str, search: &Search<'_>) -> String {
    let start = if path.is_empty() {
        search.cwd.to_string()
    } else {
        util::head(path)
    };
    find_vcs_root(&start, search)
}

pub fn discover(path: &str, search: &Search<'_>) -> Workspace {
    describe(&find(path, search))
}

pub fn describe(dir: &str) -> Workspace {
    let info = git_info(dir);
    Workspace {
        dir: dir.to_string(),
        name: info
            .as_ref()
            .map(|i| i.repo_name.clone())
            .unwrap_or_else(|| util::tail(dir)),
        worktree: info.as_ref().and_then(|i| i.worktree.clone()),
        repo_url: repo_url(dir, info.as_ref()),
    }
}

pub struct GitInfo {
    /// The shared `.git` directory.
    pub common_dir: String,
    pub repo_name: String,
    /// Name of the linked worktree directory; `None` for the main worktree.
    pub worktree: Option<String>,
}

static GITDIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"gitdir:\s*(.+)").unwrap());

fn resolve_relative(base: &str, path: &str) -> String {
    let path = path.trim();
    let joined = if Path::new(path).is_absolute() {
        path.to_string()
    } else {
        format!("{base}/{path}")
    };
    util::realpath(&joined).unwrap_or_else(|| path.to_string())
}

pub fn git_info(dir: &str) -> Option<GitInfo> {
    let dot_git = format!("{dir}/.git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(GitInfo {
            common_dir: dot_git,
            repo_name: util::tail(dir),
            worktree: None,
        });
    }
    if !meta.is_file() {
        return None;
    }
    // Linked worktree: `gitdir: <common>/worktrees/<name>`
    let content = std::fs::read_to_string(&dot_git).unwrap_or_default();
    let git_dir =
        resolve_relative(dir, GITDIR.captures(&content)?.get(1)?.as_str());
    let commondir =
        std::fs::read_to_string(format!("{git_dir}/commondir")).ok();
    let common = match &commondir {
        Some(c) => resolve_relative(&git_dir, c),
        None => git_dir,
    };
    let name = util::tail(&common);
    let repo_name = if name == ".git" {
        util::tail(&util::head(&common))
    } else {
        name.strip_suffix(".git").unwrap_or(&name).to_string()
    };
    Some(GitInfo {
        common_dir: common,
        repo_name,
        worktree: commondir.map(|_| util::tail(dir)),
    })
}

static ORIGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[remote "origin"\]\s*\n\s*url\s*=\s*([^\n]+)"#).unwrap()
});
static ANY_REMOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[remote "[^"]+"\]\s*\n\s*url\s*=\s*([^\n]+)"#).unwrap()
});
static SCP_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?)@(.+)$").unwrap());

fn repo_url(dir: &str, info: Option<&GitInfo>) -> Option<String> {
    let common = info
        .map(|i| i.common_dir.clone())
        .unwrap_or_else(|| format!("{dir}/.git"));
    let content = std::fs::read_to_string(format!("{common}/config")).ok()?;
    let url = ORIGIN
        .captures(&content)
        .or_else(|| ANY_REMOTE.captures(&content))?;
    format_url(url.get(1)?.as_str().trim())
}

/// Converts a git remote URL into a browsable https URL.
pub fn format_url(url: &str) -> Option<String> {
    if url.starts_with("http") {
        return Some(url.strip_suffix(".git").unwrap_or(url).to_string());
    }
    let caps = SCP_URL.captures(url)?;
    let repo = caps.get(2)?.as_str().replacen(':', "/", 1);
    Some(format!(
        "https://{}",
        repo.strip_suffix(".git").unwrap_or(&repo)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn urls() {
        assert_eq!(
            format_url("https://github.com/a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
        assert_eq!(
            format_url("git@github.com:a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
        assert_eq!(format_url("nonsense"), None);
    }

    #[test]
    fn repositories_and_worktrees() {
        let root = std::env::temp_dir()
            .join(format!("cord-ws-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let repo = root.join("NikSOS/master");
        let tree = root.join("NikSOS/v3");
        fs::create_dir_all(repo.join(".git/worktrees/v3")).unwrap();
        fs::create_dir_all(tree.join("src")).unwrap();
        fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:NikSneMC/NikSOS.git\n",
        )
        .unwrap();
        fs::write(repo.join(".git/worktrees/v3/commondir"), "../..\n").unwrap();
        fs::write(
            tree.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/v3").display()),
        )
        .unwrap();

        let markers = vec![".git".to_string()];
        let search = Search {
            root_markers: &markers,
            limit: None,
            cwd: "/",
        };

        let main = discover(repo.join("a.rs").to_str().unwrap(), &search);
        assert_eq!(main.name, "master");
        assert_eq!(main.worktree, None);
        assert_eq!(
            main.repo_url.as_deref(),
            Some("https://github.com/NikSneMC/NikSOS")
        );

        let linked = discover(tree.join("src/b.rs").to_str().unwrap(), &search);
        assert_eq!(linked.dir, util::normalize(tree.to_str().unwrap()));
        assert_eq!(linked.name, "master");
        assert_eq!(linked.worktree.as_deref(), Some("v3"));
        assert_eq!(
            linked.repo_url.as_deref(),
            Some("https://github.com/NikSneMC/NikSOS")
        );

        let _ = fs::remove_dir_all(&root);
    }
}
