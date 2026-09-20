//! Pure-Rust git wrapper over `gix`.
//!
//! Same surface as the prior `git2` version: open/init, commit one file with
//! attribution, list per-file history. No long-lived `Repository` handle —
//! every call re-opens. Writes serialize on `write_lock`; reads are stateless.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gix::bstr::{BStr, BString, ByteSlice};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct Repo {
    root: PathBuf,
    write_lock: Arc<Mutex<()>>,
    author_template: String,
}

/// One write order shared by Git and the page service's database projection.
pub(crate) struct WriteSession {
    repo: Repo,
    guard: Arc<OwnedMutexGuard<()>>,
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

    /// Write `content` to `relpath` (relative to repo root) and commit it,
    /// attributed to the given actor, as **one serialized operation**.
    ///
    /// Taking the content by value rather than re-reading the file is the
    /// whole point. The previous shape — caller writes the file, then asks
    /// git to commit whatever is at that path — had two defects that only
    /// appear under concurrency, and they are the kind that destroy trust in
    /// an attributed store:
    ///
    /// 1. The disk write happened *outside* this lock. Two actors saving the
    ///    same page could interleave as `A writes → B writes → A commits`,
    ///    and A's commit would carry B's bytes under A's name. Attribution
    ///    silently wrong is worse than a failed write.
    /// 2. Even serialized, re-reading reintroduces the gap: the bytes
    ///    committed were never proven to be the bytes the caller validated.
    ///
    /// Holding one lock across both, and committing the same buffer that was
    /// written, closes it: the file on disk and the blob in git are the same
    /// bytes by construction.
    pub async fn write_and_commit(
        &self,
        relpath: &Path,
        content: &str,
        actor_type: &str,
        actor_handle: &str,
        message: &str,
    ) -> Result<String> {
        self.write_session()
            .await
            .write_and_commit(relpath, content, actor_type, actor_handle, message)
            .await
    }

    pub(crate) async fn write_session(&self) -> WriteSession {
        WriteSession {
            repo: self.clone(),
            guard: Arc::new(self.write_lock.clone().lock_owned().await),
        }
    }

    /// Drain accepted writes after the server stops accepting requests.
    pub async fn wait_for_writes(&self) {
        let _guard = self.write_lock.lock().await;
    }
}

impl WriteSession {
    pub(crate) async fn write_and_commit(
        &self,
        relpath: &Path,
        content: &str,
        actor_type: &str,
        actor_handle: &str,
        message: &str,
    ) -> Result<String> {
        let root = self.repo.root.clone();
        let author = render_author(&self.repo.author_template, actor_type, actor_handle);
        let relpath = relpath.to_path_buf();
        let message = message.to_string();
        let content = content.as_bytes().to_vec();
        let guard = self.guard.clone();

        tokio::task::spawn_blocking(move || -> Result<String> {
            // Cancellation cannot stop spawn_blocking once it has started.
            // Keep the lock with the worker until its file/Git writes finish.
            let _guard = guard;
            let repo = gix::open(&root).map_err(err)?;
            let abs_path = root.join(&relpath);
            if let Some(parent) = abs_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs_path, &content)?;
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
}

impl Repo {
    /// The file's content as of `oid`, or `None` if it did not exist there.
    ///
    /// This is what makes attribution *checkable* rather than merely
    /// recorded: given a commit, you can ask what it actually says, instead
    /// of trusting that the author line and the bytes belong together.
    pub async fn file_at_commit(&self, oid: &str, relpath: &Path) -> Result<Option<String>> {
        let root = self.root.clone();
        let relpath = relpath.to_path_buf();
        let oid = oid.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            let repo = gix::open(&root).map_err(err)?;
            let id = gix::ObjectId::from_hex(oid.as_bytes()).map_err(err)?;
            let commit = repo.find_commit(id).map_err(err)?;
            let tree_id = commit.tree_id().map_err(err)?.detach();
            let target = path_to_bstring(&relpath)?;
            let Some(blob_id) = lookup_path_in_tree(&repo, tree_id, target.as_bstr())? else {
                return Ok(None);
            };
            let obj = repo.find_object(blob_id).map_err(err)?;
            Ok(Some(String::from_utf8_lossy(&obj.data).into_owned()))
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

    const TEMPLATE: &str = "{actor_type}:{actor_handle} <{actor_handle}@wikikiki.local>";

    /// The regression guard for the cross-attribution race.
    ///
    /// With the disk write outside the lock and the content re-read inside
    /// it, concurrent saves of one page interleaved as `A writes → B writes
    /// → A commits`, and A's commit carried B's bytes under A's name. Here
    /// every commit is asked what it actually contains, and it must be the
    /// content its own author submitted.
    #[tokio::test]
    async fn concurrent_writes_never_cross_attribution() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let repo = Repo::open_or_init(tmp.path(), TEMPLATE).expect("init");
        let rel = PathBuf::from("notes").join("shared.md");

        let handles = ["alice", "bob", "carol", "dave", "erin", "frank"];
        let mut tasks = Vec::new();
        for who in handles {
            let r = repo.clone();
            let rel = rel.clone();
            tasks.push(tokio::spawn(async move {
                r.write_and_commit(
                    &rel,
                    &format!("content from {who}"),
                    "human",
                    who,
                    &format!("edit by {who}"),
                )
                .await
            }));
        }
        for t in tasks {
            t.await.expect("join").expect("commit");
        }

        let history = repo.file_history(&rel, 50).await.expect("history");
        assert_eq!(history.len(), handles.len(), "one commit per writer");

        for c in &history {
            let who = c
                .author_name
                .strip_prefix("human:")
                .unwrap_or_else(|| panic!("unexpected author {}", c.author_name));
            let content = repo
                .file_at_commit(&c.oid, &rel)
                .await
                .expect("read blob")
                .expect("file exists at its own commit");
            assert_eq!(
                content,
                format!("content from {who}"),
                "commit {} attributed to {who} carries another actor's bytes",
                c.oid
            );
        }
    }

    /// Whatever the interleaving, the file left on disk must be exactly the
    /// content of the last commit — not a blend, and not a stale write that
    /// landed after the commit that was supposed to capture it.
    #[tokio::test]
    async fn disk_agrees_with_head_after_concurrent_writes() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let repo = Repo::open_or_init(tmp.path(), TEMPLATE).expect("init");
        let rel = PathBuf::from("page.md");

        let mut tasks = Vec::new();
        for i in 0..8 {
            let r = repo.clone();
            let rel = rel.clone();
            tasks.push(tokio::spawn(async move {
                r.write_and_commit(&rel, &format!("body {i}"), "agent", &format!("a{i}"), "edit")
                    .await
            }));
        }
        for t in tasks {
            t.await.expect("join").expect("commit");
        }

        let on_disk = std::fs::read_to_string(tmp.path().join(&rel)).expect("read file");
        let newest = repo.file_history(&rel, 1).await.expect("history");
        let in_git = repo
            .file_at_commit(&newest[0].oid, &rel)
            .await
            .expect("read blob")
            .expect("blob");
        assert_eq!(on_disk, in_git, "disk and HEAD disagree");
    }
}
