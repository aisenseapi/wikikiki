//! Pure-Rust git wrapper over `gix`.
//!
//! Same surface as the prior `git2` version: open/init, commit one file with
//! attribution, list per-file history. No long-lived `Repository` handle —
//! every call re-opens. Writes serialize on `write_lock`; reads are stateless.

use std::io::Write;
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
    // An OS lock complements the in-process mutex. Clones share ownership;
    // opening the same repository independently must not create a second writer.
    _process_lock: Arc<std::fs::File>,
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
        let process_lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".wikikiki-writer.lock"))?;
        process_lock.try_lock().map_err(|error| {
            AppError::Conflict(format!(
                "content repository is already in use or cannot be locked: {error}"
            ))
        })?;
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
            _process_lock: Arc::new(process_lock),
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

    pub(crate) fn validate_target(&self, relpath: &Path) -> Result<()> {
        validate_target(&self.root, relpath)
    }

    pub(crate) async fn head_oid(&self) -> Result<String> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let repo = gix::open(root).map_err(err)?;
            Ok(repo.head_id().map_err(err)?.detach().to_string())
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
    }

    /// Find a committed intent after a crash between Git and SQLite.
    pub(crate) async fn find_operation(&self, operation_id: &str) -> Result<Option<String>> {
        let root = self.root.clone();
        let operation_id = operation_id.to_string();
        tokio::task::spawn_blocking(move || {
            let repo = gix::open(root).map_err(err)?;
            let head = repo.head_id().map_err(err)?;
            for item in repo.rev_walk([head]).all().map_err(err)? {
                let item = item.map_err(err)?;
                let commit = repo.find_commit(item.id).map_err(err)?;
                let message = commit.decode().map_err(err)?.message.to_string();
                if operation_metadata(&message)
                    .is_some_and(|(operation, _)| operation == operation_id)
                {
                    return Ok(Some(item.id.to_string()));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
    }
}

impl WriteSession {
    pub(crate) fn author_signature(&self, actor_type: &str, actor_handle: &str) -> Result<String> {
        let rendered = render_author(&self.repo.author_template, actor_type, actor_handle);
        parse_sig(&rendered)?;
        Ok(rendered)
    }

    pub(crate) async fn write_and_commit(
        &self,
        relpath: &Path,
        content: &str,
        actor_type: &str,
        actor_handle: &str,
        message: &str,
    ) -> Result<String> {
        let author = self.author_signature(actor_type, actor_handle)?;
        let oid = self
            .commit_content(relpath, content, &author, message, None, None)
            .await?;
        self.materialize(relpath, content, &oid).await?;
        Ok(oid)
    }

    pub(crate) async fn commit_content(
        &self,
        relpath: &Path,
        content: &str,
        author: &str,
        message: &str,
        operation_id: Option<&str>,
        expected_head: Option<&str>,
    ) -> Result<String> {
        self.repo.validate_target(relpath)?;
        let root = self.repo.root.clone();
        let (name, email) = parse_sig(author)?;
        let relpath = relpath.to_path_buf();
        let message = match operation_id {
            Some(operation) => format!(
                "{message}\n\nWikikiki-Operation: {operation}\nWikikiki-Path: {}\n",
                relpath.to_string_lossy().replace('\\', "/")
            ),
            None => message.to_string(),
        };
        let expected_head = expected_head.map(str::to_string);
        let content = content.as_bytes().to_vec();
        let guard = self.guard.clone();

        tokio::task::spawn_blocking(move || -> Result<String> {
            // Cancellation cannot stop spawn_blocking once it has started.
            // Keep the lock with the worker until its file/Git writes finish.
            let _guard = guard;
            let repo = gix::open(&root).map_err(err)?;
            let parent_id = repo.head_id().map_err(err)?.detach();
            if expected_head
                .as_deref()
                .is_some_and(|expected| expected != parent_id.to_string())
            {
                return Err(AppError::Conflict(
                    "Git HEAD changed outside the pending write; recovery requires operator review"
                        .into(),
                ));
            }
            let blob_id = repo.write_blob(&content).map_err(err)?.detach();

            // Build the new tree by editing the parent commit's tree (or
            // an empty tree if there is no parent yet). This bypasses the
            // working-tree/index plumbing entirely — wikikiki has no
            // checkout, so the index would only be incidental state.
            let parent_id_opt = Some(parent_id);
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
            let sig = make_signature(&name, &email);
            let commit_id = repo
                .commit_as(&sig, &sig, "HEAD", message.clone(), new_tree_id, parent_ids)
                .map_err(err)?;

            Ok(commit_id.detach().to_string())
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
    }

    /// Publish a complete file only after its immutable Git object exists.
    /// Repeating this operation is safe after a failed projection or restart.
    pub(crate) async fn materialize(
        &self,
        relpath: &Path,
        content: &str,
        operation_id: &str,
    ) -> Result<()> {
        let root = self.repo.root.clone();
        let relpath = relpath.to_path_buf();
        let content = content.as_bytes().to_vec();
        let operation_id = operation_id.to_string();
        let guard = self.guard.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            validate_target(&root, &relpath)?;
            let destination = root.join(&relpath);
            let parent = destination
                .parent()
                .ok_or_else(|| AppError::BadRequest("page has no parent directory".into()))?;
            std::fs::create_dir_all(parent)?;
            let temporary = parent.join(format!(".wikikiki-write-{operation_id}.tmp"));
            // An interrupted attempt may have left its own temporary file.
            if temporary.exists() {
                let metadata = std::fs::symlink_metadata(&temporary)?;
                if is_link(&metadata) || !metadata.is_file() {
                    return Err(AppError::Conflict("unsafe recovery temporary file".into()));
                }
                std::fs::remove_file(&temporary)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&content)?;
            file.sync_all()?;
            drop(file);
            validate_target(&root, &relpath)?;
            std::fs::rename(&temporary, &destination)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
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
            Ok(Some(String::from_utf8(obj.data.clone()).map_err(|_| {
                AppError::Conflict("historical page is not valid UTF-8".into())
            })?))
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
    let s = p
        .to_str()
        .ok_or_else(|| AppError::BadRequest(format!("non-utf8 path: {}", p.display())))?;
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
    let starting_id =
        parent_tree_id.unwrap_or_else(|| gix::ObjectId::empty_tree(repo.object_hash()));
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
    // Metadata-only writes have the same blob but are still attributed edits.
    let decoded = commit.decode().map_err(err)?;
    let message = decoded.message.to_string();
    if operation_metadata(&message).is_some_and(|(_, path)| path == target) {
        return Ok(true);
    }
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

/// Only the final generated trailer pair is authoritative. User summaries
/// may contain arbitrary lines, including apparent trailers for other pages.
fn operation_metadata(message: &str) -> Option<(&str, &str)> {
    let mut lines = message.trim_end_matches('\n').lines().rev();
    let path = lines.next()?.strip_prefix("Wikikiki-Path: ")?;
    let operation = lines.next()?.strip_prefix("Wikikiki-Operation: ")?;
    if path.is_empty()
        || operation.len() != 64
        || !operation.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((operation, path))
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

fn parse_sig(rendered: &str) -> Result<(String, String)> {
    let invalid = || AppError::BadRequest("invalid Git author signature".into());
    if rendered.chars().any(char::is_control) {
        return Err(invalid());
    }
    let (name, email) = match (rendered.find('<'), rendered.find('>')) {
        (Some(open), Some(close))
            if open < close
                && !rendered[..open].contains('>')
                && !rendered[open + 1..close].contains(['<', '>'])
                && rendered[close + 1..].trim().is_empty() =>
        {
            (rendered[..open].trim(), rendered[open + 1..close].trim())
        }
        (None, None) => (rendered.trim(), "anonymous@wikikiki.local"),
        _ => return Err(invalid()),
    };
    if name.is_empty() || email.is_empty() || email.chars().any(char::is_whitespace) {
        return Err(invalid());
    }
    Ok((name.to_string(), email.to_string()))
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Reject symlinks, reparse points, case aliases and file/directory collisions
/// before content or history is changed. This also protects existing imports.
pub(crate) fn validate_target(root: &Path, relpath: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    let components: Vec<_> = relpath.components().collect();
    if components.is_empty() {
        return Err(AppError::BadRequest("empty content path".into()));
    }
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(segment) = component else {
            return Err(AppError::BadRequest("content path must be relative".into()));
        };
        let name = segment.to_string_lossy();
        if name.eq_ignore_ascii_case(".git") {
            return Err(AppError::BadRequest(
                "Git metadata is not wiki content".into(),
            ));
        }
        if current.is_dir() {
            for entry in std::fs::read_dir(&current)? {
                let actual = entry?.file_name();
                if actual.to_string_lossy().eq_ignore_ascii_case(&name)
                    && actual.as_os_str() != *segment
                {
                    return Err(AppError::Conflict(format!(
                        "content path aliases existing name {}",
                        actual.to_string_lossy()
                    )));
                }
            }
        }
        current.push(segment);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_link(&metadata) {
                    return Err(AppError::Conflict(
                        "symlinks and reparse points are not supported in content paths".into(),
                    ));
                }
                let last = index + 1 == components.len();
                if (last && !metadata.is_file()) || (!last && !metadata.is_dir()) {
                    return Err(AppError::Conflict(
                        "content path collides with a file or directory".into(),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
        let (n, e) = parse_sig("human:alice <alice@wikikiki.local>").expect("valid signature");
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
                r.write_and_commit(
                    &rel,
                    &format!("body {i}"),
                    "agent",
                    &format!("a{i}"),
                    "edit",
                )
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
