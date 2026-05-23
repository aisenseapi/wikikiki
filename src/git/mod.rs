//! Pure-Rust git wrapper over `gix`.
//!
//! Same surface as the prior `git2` version: open/init, commit one file with
//! attribution, list per-file history. No long-lived `Repository` handle —
//! every call re-opens. Writes serialize on `write_lock`; reads are stateless.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gix::bstr::{BStr, BString, ByteSlice};
use tokio::sync::Mutex;

use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct Repo {
    root: PathBuf,
    write_lock: Arc<Mutex<()>>,
    author_template: String,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub oid: String,
    pub author_name: String,
    pub author_email: String,
    pub message: String,
    pub time: i64,
}

fn err<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Internal(format!("git: {e}"))
}

impl Repo {
    /// Initialise the repo at `root` if missing, otherwise open it. Ensures
    /// at least one commit exists so HEAD is always resolvable.
    pub fn open_or_init(root: &Path, author_template: &str) -> Result<Self> {
        if !root.exists() {
            std::fs::create_dir_all(root)?;
        }
        let has_repo = root.join(".git").exists();
        if !has_repo {
            let repo = gix::init(root).map_err(err)?;
            seed_initial_commit(&repo, root)?;
        } else {
            // Verify it opens; we don't keep the handle.
            gix::open(root).map_err(err)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            write_lock: Arc::new(Mutex::new(())),
            author_template: author_template.to_string(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Commit `relpath` (relative to repo root) with the given attribution.
    /// The file must already exist on disk at its final location.
    pub async fn commit_file(
        &self,
        relpath: &Path,
        actor_type: &str,
        actor_handle: &str,
        message: &str,
    ) -> Result<String> {
        let _guard = self.write_lock.lock().await;
        let root = self.root.clone();
        let author = render_author(&self.author_template, actor_type, actor_handle);
        let relpath = relpath.to_path_buf();
        let message = message.to_string();

        tokio::task::spawn_blocking(move || -> Result<String> {
            let repo = gix::open(&root).map_err(err)?;
            let abs_path = root.join(&relpath);
            let content = std::fs::read(&abs_path)?;
            let blob_id = repo.write_blob(&content).map_err(err)?.detach();

            // Build the new tree by editing the parent commit's tree (or
            // an empty tree if there is no parent yet). This bypasses the
            // working-tree/index plumbing entirely — wikikiki has no
            // checkout, so the index would only be incidental state.
            let parent_id_opt = repo.head_id().ok().map(|id| id.detach());
            let parent_tree_id = match parent_id_opt {
                Some(commit_id) => {
                    let commit = repo.find_commit(commit_id).map_err(err)?;
                    Some(commit.tree_id().map_err(err)?.detach())
                }
                None => None,
            };
            let path_bs = path_to_bstring(&relpath)?;
            let new_tree_id = edit_tree_with_blob(&repo, parent_tree_id, &path_bs, blob_id)?;

            let parent_ids: Vec<gix::ObjectId> = parent_id_opt.into_iter().collect();
            let (name, email) = parse_sig(&author);
            let sig = make_signature(&name, &email);
            let commit_id = repo
                .commit_as(&sig, &sig, "HEAD", message.clone(), new_tree_id, parent_ids)
                .map_err(err)?;

            Ok(commit_id.detach().to_string())
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
    }

    /// List commits that touched `relpath`, newest first. Bounded by `limit`.
    pub async fn file_history(&self, relpath: &Path, limit: usize) -> Result<Vec<CommitInfo>> {
        let root = self.root.clone();
        let relpath = relpath.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<Vec<CommitInfo>> {
            let repo = gix::open(&root).map_err(err)?;
            let head_id = match repo.head_id() {
                Ok(id) => id,
                Err(_) => return Ok(Vec::new()),
            };
            let target = path_to_bstring(&relpath)?;
            let mut out = Vec::new();
            let walk = repo.rev_walk([head_id]).all().map_err(err)?;
            for info in walk {
                let info = info.map_err(err)?;
                let commit = repo.find_commit(info.id).map_err(err)?;
                if commit_touched(&repo, &commit, target.as_bstr())? {
                    let decoded = commit.decode().map_err(err)?;
                    let author = decoded.author;
                    out.push(CommitInfo {
                        oid: info.id.to_string(),
                        author_name: author.name.to_string(),
                        author_email: author.email.to_string(),
                        message: decoded.message().title.to_string(),
                        time: author.time.seconds,
                    });
                    if out.len() >= limit {
                        break;
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
    }
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn seed_initial_commit(repo: &gix::Repository, root: &Path) -> Result<()> {
    let seed = root.join(".keep");
    if !seed.exists() {
        std::fs::write(&seed, b"")?;
    }
    let blob_id = repo.write_blob(b"").map_err(err)?.detach();
    let path_bs = path_to_bstring(Path::new(".keep"))?;
    let tree_id = edit_tree_with_blob(repo, None, &path_bs, blob_id)?;
    let sig = make_signature("wikikiki", "init@wikikiki.local");
    repo.commit_as(
        &sig,
        &sig,
        "HEAD",
        "init: wikikiki content repo",
        tree_id,
        Vec::<gix::ObjectId>::new(),
    )
    .map_err(err)?;
    Ok(())
}

fn make_signature(name: &str, email: &str) -> gix::actor::Signature {
    gix::actor::Signature {
        name: name.into(),
        email: email.into(),
        time: gix::date::Time::now_local_or_utc(),
    }
}

/// Convert a relative path (no leading slash, '/'-separated) into a
/// gix-friendly BString. Backslashes are normalised to forward slashes.
fn path_to_bstring(p: &Path) -> Result<BString> {
    let s = p.to_str().ok_or_else(|| {
        AppError::BadRequest(format!("non-utf8 path: {}", p.display()))
    })?;
    Ok(BString::from(s.replace('\\', "/")))
}

/// Build a new tree object by taking `parent_tree_id` (or an empty tree
/// if None) and upserting a blob at `path`. Nested-path segments are
/// created or updated as needed by gix's editor.
fn edit_tree_with_blob(
    repo: &gix::Repository,
    parent_tree_id: Option<gix::ObjectId>,
    path: &BString,
    blob_id: gix::ObjectId,
) -> Result<gix::ObjectId> {
    let starting_id = parent_tree_id
        .unwrap_or_else(|| gix::ObjectId::empty_tree(repo.object_hash()));
    let mut editor = repo.edit_tree(starting_id).map_err(err)?;
    editor
        .upsert(path.as_bstr(), gix::object::tree::EntryKind::Blob, blob_id)
        .map_err(err)?;
    let new_oid = editor.write().map_err(err)?.detach();
    Ok(new_oid)
}

/// Did this commit change the file at `target` relative to its first parent?
/// First-parent only — sufficient for our linear-history use case.
fn commit_touched(repo: &gix::Repository, commit: &gix::Commit, target: &BStr) -> Result<bool> {
    let this_tree_id = commit.tree_id().map_err(err)?;
    let this_blob = lookup_path_in_tree(repo, this_tree_id.detach(), target)?;
    let parents: Vec<_> = commit.parent_ids().collect();
    if parents.is_empty() {
        return Ok(this_blob.is_some());
    }
    let parent_commit = repo.find_commit(parents[0]).map_err(err)?;
    let parent_tree_id = parent_commit.tree_id().map_err(err)?;
    let parent_blob = lookup_path_in_tree(repo, parent_tree_id.detach(), target)?;
    Ok(this_blob != parent_blob)
}

/// Walk into nested trees and return the blob OID at `path`, if any.
fn lookup_path_in_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    path: &BStr,
) -> Result<Option<gix::ObjectId>> {
    let mut current_tree_id = tree_id;
    let mut segments = path.split(|&b| b == b'/').peekable();
    while let Some(seg) = segments.next() {
        let tree = repo.find_tree(current_tree_id).map_err(err)?;
        let entries = tree.iter();
        let mut matched = None;
        for entry_res in entries {
            let entry = entry_res.map_err(err)?;
            if entry.filename() == seg {
                matched = Some((entry.oid().to_owned(), entry.mode()));
                break;
            }
        }
        match matched {
            None => return Ok(None),
            Some((oid, mode)) => {
                if segments.peek().is_none() {
                    // Final segment — must be a file/blob.
                    if mode.is_blob() {
                        return Ok(Some(oid));
                    } else {
                        return Ok(None);
                    }
                } else {
                    // More to walk — must be a tree.
                    if mode.is_tree() {
                        current_tree_id = oid;
                    } else {
                        return Ok(None);
                    }
                }
            }
        }
    }
    Ok(None)
}

fn render_author(template: &str, actor_type: &str, actor_handle: &str) -> String {
    template
        .replace("{actor_type}", actor_type)
        .replace("{actor_handle}", actor_handle)
}

fn parse_sig(rendered: &str) -> (String, String) {
    if let Some(open) = rendered.find('<') {
        if let Some(close) = rendered.find('>') {
            let name = rendered[..open].trim().to_string();
            let email = rendered[open + 1..close].to_string();
            return (name, email);
        }
    }
    (rendered.to_string(), "anonymous@wikikiki.local".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_author_template() {
        let s = render_author(
            "{actor_type}:{actor_handle} <{actor_handle}@wikikiki.local>",
            "human",
            "alice",
        );
        assert_eq!(s, "human:alice <alice@wikikiki.local>");
    }

    #[test]
    fn parse_sig_basic() {
        let (n, e) = parse_sig("human:alice <alice@wikikiki.local>");
        assert_eq!(n, "human:alice");
        assert_eq!(e, "alice@wikikiki.local");
    }
}
