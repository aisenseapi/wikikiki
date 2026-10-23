#!/usr/bin/env python3
"""Publish top-level Markdown files with persistent optimistic concurrency.

Python 3.10+ standard library only. See docs/AGENT_API.md for setup and recovery.
"""

import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

MAX_BODY = 4 * 1024 * 1024
SEGMENT = re.compile(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*\Z")


class SyncError(Exception):
    pass


class NoRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # A redirect must never send a bearer token to a different destination.
        return None


def valid_segment(value):
    stem = value.split(".", 1)[0].upper()
    reserved = stem in {"CON", "PRN", "AUX", "NUL"} or bool(re.fullmatch(r"(?:COM|LPT)[1-9]", stem))
    return bool(SEGMENT.fullmatch(value)) and len(value) <= 64 and not value.endswith(".") and not reserved


def file_identity(info):
    return info.st_dev, info.st_ino


def read_regular_file(path, limit):
    """Reject symlinks and changing files before any bytes leave this process."""
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise SyncError(f"{path.name}: only regular files with one link are accepted")
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    fd = os.open(path, flags)
    with os.fdopen(fd, "rb") as source:
        opened = os.fstat(source.fileno())
        if not stat.S_ISREG(opened.st_mode) or file_identity(opened) != file_identity(before):
            raise SyncError(f"{path.name}: file changed while opening")
        body = source.read(limit + 1)
        after = os.fstat(source.fileno())
    current = path.lstat()
    if (
        file_identity(current) != file_identity(opened)
        or not stat.S_ISREG(current.st_mode)
        or current.st_nlink != 1
        or (after.st_size, after.st_mtime_ns) != (opened.st_size, opened.st_mtime_ns)
        or (current.st_size, current.st_mtime_ns) != (after.st_size, after.st_mtime_ns)
    ):
        raise SyncError(f"{path.name}: file changed while reading, retry after saving")
    if len(body) > limit:
        raise SyncError(f"{path.name}: exceeds the {limit}-byte limit")
    return body


class StateLock:
    def __init__(self, path):
        self.path = path
        self.file = None

    def __enter__(self):
        if self.path.is_symlink():
            raise SyncError("sync lock must not be a symlink")
        flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(self.path, flags, 0o600)
        self.file = os.fdopen(fd, "r+b")
        try:
            if os.name == "nt":
                import msvcrt

                if os.fstat(fd).st_size == 0:
                    self.file.write(b"0")
                    self.file.flush()
                self.file.seek(0)
                msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
            else:
                import fcntl

                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.file.close()
            raise SyncError("another sync process is using this state") from None
        return self

    def __exit__(self, *args):
        self.file.close()


class SyncClient:
    def __init__(self, base, token, workspace, handle, state_dir, timeout=20):
        self.base = base.rstrip("/")
        parsed = urllib.parse.urlsplit(self.base)
        if (
            parsed.scheme not in ("http", "https")
            or not parsed.hostname
            or parsed.username is not None
            or parsed.password is not None
            or parsed.query
            or parsed.fragment
            or any(char.isspace() for char in self.base)
        ):
            raise SyncError("WIKIKIKI_BASE must be an HTTP(S) URL without credentials, query or fragment")
        if not token or any(char.isspace() for char in token):
            raise SyncError("WIKIKIKI_TOKEN must be a nonempty token without whitespace")
        if not valid_segment(handle):
            raise SyncError("ACTOR_HANDLE must be one portable ASCII path segment, at most 64 characters")
        self.token = token
        self.workspace = Path(workspace).resolve(strict=True)
        if not self.workspace.is_dir():
            raise SyncError("WORKSPACE must be a directory")
        self.workspace_id = file_identity(self.workspace.lstat())
        self.handle = handle
        self.timeout = timeout
        self.opener = urllib.request.build_opener(NoRedirects())
        self.state_dir = Path(state_dir).expanduser().absolute()
        if self.state_dir.is_symlink():
            raise SyncError("state directory must not be a symlink")
        self.state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.scope = [self.base, str(self.workspace), self.handle]
        key = hashlib.sha256(json.dumps(self.scope).encode()).hexdigest()
        self.state_path = self.state_dir / (key + ".json")
        self.lock_path = self.state_dir / (key + ".lock")
        self.entries = {}

    def load_state(self):
        try:
            state = json.loads(read_regular_file(self.state_path, 8 * 1024 * 1024))
        except FileNotFoundError:
            self.entries = {}
            return
        except (ValueError, UnicodeError):
            raise SyncError("sync state is invalid, restore it from backup or reconcile pages manually") from None
        if not isinstance(state, dict) or state.get("version") != 1 or state.get("scope") != self.scope:
            raise SyncError("sync state has an unsupported format or a different scope")
        entries = state.get("entries")
        if not isinstance(entries, dict) or any(
            not isinstance(entry, dict)
            or not isinstance(entry.get("etag"), str)
            or not isinstance(entry.get("sha256"), str)
            for entry in entries.values()
        ):
            raise SyncError("sync state entries are invalid")
        self.entries = entries

    def save_state(self):
        fd, name = tempfile.mkstemp(prefix=".sync-", dir=self.state_dir)
        try:
            with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as target:
                json.dump({"version": 1, "scope": self.scope, "entries": self.entries}, target)
                target.write("\n")
                target.flush()
                os.fsync(target.fileno())
            os.replace(name, self.state_path)
        finally:
            if os.path.exists(name):
                os.unlink(name)

    def request(self, method, page, body=None, condition=None):
        headers = {"Authorization": "Bearer " + self.token, "Accept": "text/markdown"}
        if body is not None:
            headers["Content-Type"] = "text/markdown"
            headers["X-Summary"] = "Workspace sync"
        if condition:
            headers[condition[0]] = condition[1]
        request = urllib.request.Request(
            self.base + "/api/pages/" + page, data=body, headers=headers, method=method
        )
        try:
            try:
                response = self.opener.open(request, timeout=self.timeout)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                data = response.read(MAX_BODY + 1)
                if len(data) > MAX_BODY:
                    raise SyncError(f"{page}: server response exceeds the page size limit")
                return response.code, response.headers, data
        except (urllib.error.URLError, http.client.HTTPException, TimeoutError, OSError):
            # Do not print URLs, request headers or response bodies that could contain secrets.
            raise SyncError(f"{page}: {method} transport failure, write outcome may be unknown") from None

    @staticmethod
    def etag(headers, page):
        etag = headers.get("ETag", "")
        if len(etag) > 512 or not re.fullmatch(r'"[^"\r\n]+"', etag):
            raise SyncError(f"{page}: missing or invalid strong ETag, refusing an unsafe write")
        return etag

    def sync_file(self, path):
        if not valid_segment(path.stem):
            raise SyncError(f"{path.name!r}: unsupported filename, use ASCII letters, digits, -, _ or .")
        directory = self.workspace.lstat()
        if not stat.S_ISDIR(directory.st_mode) or file_identity(directory) != self.workspace_id:
            raise SyncError("workspace directory changed, restart after checking its path")
        body = read_regular_file(path, MAX_BODY)
        if file_identity(self.workspace.lstat()) != self.workspace_id:
            raise SyncError("workspace directory changed while reading")
        try:
            body.decode("utf-8")
        except UnicodeError:
            raise SyncError(f"{path.name}: Markdown must be UTF-8") from None
        page = f"agents/{self.handle}/{path.stem}"
        digest = hashlib.sha256(body).hexdigest()
        previous = self.entries.get(page)
        if previous and previous["sha256"] == digest:
            return
        code, headers, remote = self.request("GET", page)
        if code == 200:
            current = self.etag(headers, page)
            if not headers.get("Content-Type", "").lower().startswith("text/markdown"):
                raise SyncError(f"{page}: GET did not return Markdown")
            if remote == body:
                self.entries[page] = {"etag": current, "sha256": digest}
                self.save_state()
                print(f"unchanged {page}", flush=True)
                return
            if not previous or current != previous["etag"]:
                raise SyncError(f"{page}: remote content differs from the last synced revision, reconcile manually")
            condition = ("If-Match", current)
        elif code == 404 and not previous:
            condition = ("If-None-Match", "*")
        elif code == 404:
            raise SyncError(f"{page}: previously synced page is missing, refusing to recreate it")
        else:
            raise SyncError(f"{page}: GET HTTP {code}, no write attempted")
        code, headers, _ = self.request("PUT", page, body, condition)
        if code == 412:
            raise SyncError(f"{page}: HTTP 412, another writer changed the page, reconcile manually")
        if code != 200:
            raise SyncError(f"{page}: PUT HTTP {code}, state not advanced, check the remote page")
        self.entries[page] = {"etag": self.etag(headers, page), "sha256": digest}
        self.save_state()
        print(f"synced {page}", flush=True)

    def sync_once(self):
        failures = 0
        files = sorted(path for path in self.workspace.iterdir() if path.suffix == ".md")
        names = {}
        for path in files:
            key = path.stem.lower()
            if key in names:
                print(f"error: case-colliding filenames {names[key]!r} and {path.name!r}, no files published",
                      file=sys.stderr, flush=True)
                return False
            names[key] = path.name
        for path in files:
            try:
                self.sync_file(path)
            except (SyncError, OSError) as error:
                failures += 1
                message = str(error) if isinstance(error, SyncError) else f"{path.name!r}: local file access failed"
                print(f"error: {message}", file=sys.stderr, flush=True)
        return failures == 0


def default_state_dir():
    if os.name == "nt":
        return Path(os.environ.get("LOCALAPPDATA", str(Path.home()))) / "wikikiki-sync"
    return Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))) / "wikikiki-sync"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--once", action="store_true", help="scan once and exit nonzero on any failure")
    parser.add_argument("--interval", type=float, default=5, help="seconds between scans, default 5")
    args = parser.parse_args()
    try:
        if args.interval < 1 or not args.interval < float("inf"):
            raise SyncError("--interval must be a finite number of at least 1 second")
        for variable in ("WIKIKIKI_BASE", "WIKIKIKI_TOKEN", "WORKSPACE"):
            if not os.environ.get(variable):
                raise SyncError(f"{variable} is required")
        client = SyncClient(
            os.environ["WIKIKIKI_BASE"], os.environ["WIKIKIKI_TOKEN"], os.environ["WORKSPACE"],
            os.environ.get("ACTOR_HANDLE", "openclaw"),
            os.environ.get("WIKIKIKI_STATE_DIR", str(default_state_dir())),
        )
        with StateLock(client.lock_path):
            client.load_state()
            while True:
                succeeded = client.sync_once()
                if args.once:
                    return 0 if succeeded else 1
                if not succeeded:
                    # Stop on conflicts and errors instead of repeatedly writing or flooding logs.
                    print("sync stopped, resolve the reported errors before restarting", file=sys.stderr)
                    return 1
                time.sleep(args.interval)
    except SyncError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError):
        print("error: local state, workspace or URL could not be opened, check configuration", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    sys.exit(main())
