"""Run with python -B -m unittest discover -s scripts/tests -v."""

import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.dont_write_bytecode = True
SCRIPT = Path(__file__).resolve().parents[1] / "wikikiki-sync.py"
SPEC = importlib.util.spec_from_file_location("wikikiki_sync", SCRIPT)
sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync)
PAGE = "/api/pages/agents/worker/MEMORY"
TOKEN = "test-token-must-never-appear-in-output"


def revision(body):
    return '"' + hashlib.sha256(body).hexdigest() + '"'


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, code, body=b"", headers=None):
        self.send_response(code)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.server.calls.append(("GET", self.path, dict(self.headers), b""))
        if self.headers.get("Authorization") != "Bearer " + TOKEN:
            self.reply(401)
            return
        if self.server.get_status != 200:
            self.reply(self.server.get_status, TOKEN.encode(), self.server.extra_headers)
            return
        body = self.server.pages.get(self.path)
        if body is None:
            self.reply(404)
            return
        headers = {"Content-Type": "text/markdown", "ETag": revision(body)}
        headers.update(self.server.extra_headers)
        self.reply(200, body, headers)

    def do_PUT(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.calls.append(("PUT", self.path, dict(self.headers), body))
        if self.server.before_put:
            self.server.before_put(self.server)
        if self.server.put_status != 200:
            if self.server.commit_before_error:
                self.server.pages[self.path] = body
            self.reply(self.server.put_status, TOKEN.encode())
            return
        existing = self.server.pages.get(self.path)
        if self.headers.get("If-None-Match") == "*":
            matches = existing is None
        else:
            matches = existing is not None and self.headers.get("If-Match") == revision(existing)
        if not matches:
            self.reply(412)
            return
        self.server.pages[self.path] = body
        self.reply(200, b'{"ok":true}', {"ETag": revision(body)})


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        self.state = self.root / "state"
        self.file = self.workspace / "MEMORY.md"
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.pages = {}
        self.server.calls = []
        self.server.get_status = 200
        self.server.put_status = 200
        self.server.extra_headers = {}
        self.server.before_put = None
        self.server.commit_before_error = False
        thread = threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
        thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.base = f"http://127.0.0.1:{self.server.server_port}"

    def client(self, handle="worker"):
        client = sync.SyncClient(self.base, TOKEN, self.workspace, handle, self.state)
        client.load_state()
        return client

    def run_sync(self, client=None):
        output, errors = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            result = (client or self.client()).sync_once()
        self.assertNotIn(TOKEN, output.getvalue() + errors.getvalue())
        return result, errors.getvalue()

    def puts(self):
        return [call for call in self.server.calls if call[0] == "PUT"]

    def test_cli_preserves_at_prefix_crlf_unicode_and_trailing_newlines(self):
        secret = self.root / "secret.txt"
        secret.write_text("must-not-upload", encoding="utf-8")
        body = ("@" + str(secret) + "\r\n# Caf\u00e9\r\n\r\n\n").encode()
        self.file.write_bytes(body)
        env = dict(os.environ, WIKIKIKI_BASE=self.base, WIKIKIKI_TOKEN=TOKEN,
                   WORKSPACE=str(self.workspace), ACTOR_HANDLE="worker", WIKIKIKI_STATE_DIR=str(self.state))
        result = subprocess.run([sys.executable, "-B", str(SCRIPT), "--once"], env=env,
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.server.pages[PAGE], body)
        self.assertNotIn(TOKEN, result.stdout + result.stderr)
        self.assertEqual(self.puts()[0][2]["If-None-Match"], "*")

    def test_known_revision_is_used_after_restart(self):
        self.file.write_bytes(b"first\n")
        self.assertTrue(self.run_sync()[0])
        self.file.write_bytes(b"second\n")
        self.assertTrue(self.run_sync()[0])
        self.assertEqual(self.puts()[1][2]["If-Match"], revision(b"first\n"))
        self.assertEqual(self.server.pages[PAGE], b"second\n")

    def test_remote_edit_is_not_overwritten_after_restart(self):
        self.file.write_bytes(b"first\n")
        self.assertTrue(self.run_sync()[0])
        self.server.pages[PAGE] = b"human correction\n"
        self.file.write_bytes(b"local change\n")
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(len(self.puts()), 1)
        self.assertEqual(self.server.pages[PAGE], b"human correction\n")

    def test_initial_different_remote_page_is_not_overwritten(self):
        self.file.write_bytes(b"local\n")
        self.server.pages[PAGE] = b"existing\n"
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.puts(), [])

    def test_identical_remote_page_is_adopted_without_a_write(self):
        self.file.write_bytes(b"same\n\n")
        self.server.pages[PAGE] = self.file.read_bytes()
        self.assertTrue(self.run_sync()[0])
        self.assertEqual(self.puts(), [])
        self.file.write_bytes(b"new\n")
        self.assertTrue(self.run_sync()[0])
        self.assertEqual(self.puts()[0][2]["If-Match"], revision(b"same\n\n"))

    def test_unchanged_local_file_does_not_make_duplicate_writes(self):
        self.file.write_bytes(b"same\n")
        self.assertTrue(self.run_sync()[0])
        self.assertTrue(self.run_sync()[0])
        self.assertEqual(len(self.puts()), 1)

    def test_create_race_returns_conflict_without_overwrite(self):
        self.file.write_bytes(b"local\n")
        self.server.before_put = lambda server: server.pages.update({PAGE: b"other writer\n"})
        succeeded, error = self.run_sync()
        self.assertFalse(succeeded)
        self.assertIn("412", error)
        self.assertEqual(self.server.pages[PAGE], b"other writer\n")
        self.assertEqual(self.client().entries, {})

    def test_update_race_keeps_previous_state(self):
        self.file.write_bytes(b"first\n")
        self.assertTrue(self.run_sync()[0])
        self.file.write_bytes(b"second\n")
        self.server.before_put = lambda server: server.pages.update({PAGE: b"other writer\n"})
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.client().entries[PAGE.removeprefix("/api/pages/")]["etag"], revision(b"first\n"))

    def test_get_errors_never_become_page_content_or_a_write(self):
        self.file.write_bytes(b"local\n")
        for status in (401, 403, 429, 500, 503):
            with self.subTest(status=status):
                self.server.get_status = status
                self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.puts(), [])

    def test_redirect_is_not_followed(self):
        self.file.write_bytes(b"local\n")
        self.server.get_status = 302
        self.server.extra_headers = {"Location": self.base + "/destination"}
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(len(self.server.calls), 1)

    def test_failed_put_does_not_advance_state_or_retry(self):
        self.file.write_bytes(b"local\n")
        self.server.put_status = 503
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(len(self.puts()), 1)
        self.assertEqual(self.client().entries, {})

    def test_unknown_write_outcome_is_reconciled_by_bytes_on_next_run(self):
        self.file.write_bytes(b"local\n")
        self.server.put_status = 503
        self.server.commit_before_error = True
        self.assertFalse(self.run_sync()[0])
        self.server.put_status = 200
        self.assertTrue(self.run_sync()[0])
        self.assertEqual(len(self.puts()), 1)

    def test_missing_known_remote_page_is_not_recreated(self):
        self.file.write_bytes(b"first\n")
        self.assertTrue(self.run_sync()[0])
        self.server.pages.clear()
        self.file.write_bytes(b"second\n")
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(len(self.puts()), 1)

    def test_invalid_etag_is_refused(self):
        self.file.write_bytes(b"local\n")
        self.server.pages[PAGE] = b"local\n"
        self.server.extra_headers = {"ETag": 'W/"weak"'}
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.puts(), [])

    def test_non_markdown_response_is_refused(self):
        self.file.write_bytes(b"local\n")
        self.server.pages[PAGE] = b"local\n"
        self.server.extra_headers = {"Content-Type": "text/html"}
        self.assertFalse(self.run_sync()[0])

    def test_case_is_preserved_in_page_paths(self):
        self.file.write_bytes(b"upper\n")
        self.assertTrue(self.run_sync()[0])
        self.assertIn(PAGE, self.server.pages)
        self.assertNotIn(PAGE.lower(), self.server.pages)

    def test_unsafe_names_and_actor_paths_are_rejected(self):
        (self.workspace / "name with spaces.md").write_bytes(b"local\n")
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.server.calls, [])
        for handle in ("../admin", "one/two", ".", "..", "x?y", "CON", "aux.txt", "COM1", "trailing.", "x" * 65):
            with self.subTest(handle=handle), self.assertRaises(sync.SyncError):
                self.client(handle)

    def test_case_collisions_are_rejected_before_any_request(self):
        self.file.write_bytes(b"first\n")
        lower = self.workspace / "memory.md"
        if lower.exists():
            self.skipTest("filesystem is case-insensitive")
        lower.write_bytes(b"second\n")
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.server.calls, [])

    def test_case_collision_preflight_on_all_platforms(self):
        client = self.client()
        candidates = [self.file, self.workspace / "memory.md"]
        with mock.patch.object(Path, "iterdir", return_value=iter(candidates)):
            self.assertFalse(self.run_sync(client)[0])
        self.assertEqual(self.server.calls, [])

    def test_symlink_is_never_read_or_uploaded(self):
        target = self.root / "secret.txt"
        target.write_bytes(b"secret\n")
        try:
            self.file.symlink_to(target)
        except OSError:
            # Windows may require Developer Mode. The same branch is also tested below.
            self.skipTest("OS does not permit symlink creation")
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.server.calls, [])

    def test_symlink_mode_is_rejected_without_open_on_all_platforms(self):
        with mock.patch.object(Path, "lstat") as lstat, mock.patch.object(os, "open") as opened:
            lstat.return_value.st_mode = 0o120777
            with self.assertRaises(sync.SyncError):
                sync.read_regular_file(self.file, 100)
            opened.assert_not_called()

    def test_hardlink_is_rejected(self):
        target = self.root / "secret.txt"
        target.write_bytes(b"secret\n")
        os.link(target, self.file)
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.server.calls, [])

    def test_directory_and_non_utf8_are_rejected(self):
        self.file.mkdir()
        self.assertFalse(self.run_sync()[0])
        self.file.rmdir()
        self.file.write_bytes(b"\xff\xfe")
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.server.calls, [])

    def test_large_file_is_rejected_before_request(self):
        self.file.write_bytes(b"a" * (sync.MAX_BODY + 1))
        self.assertFalse(self.run_sync()[0])
        self.assertEqual(self.server.calls, [])

    def test_corrupt_state_never_silently_resets(self):
        client = self.client()
        client.state_path.write_text("not json", encoding="utf-8")
        with self.assertRaises(sync.SyncError):
            self.client()

    def test_state_contains_revisions_but_no_token_or_content(self):
        self.file.write_bytes(b"private page contents\n")
        self.assertTrue(self.run_sync()[0])
        state = self.client().state_path.read_text(encoding="utf-8")
        self.assertNotIn(TOKEN, state)
        self.assertNotIn("private page contents", state)
        self.assertEqual(json.loads(state)["version"], 1)

    def test_only_one_process_can_use_a_state_file(self):
        client = self.client()
        with sync.StateLock(client.lock_path):
            with self.assertRaises(sync.SyncError):
                with sync.StateLock(client.lock_path):
                    self.fail("second lock succeeded")

    def test_transport_failure_is_not_retried_or_logged_with_token(self):
        client = self.client()
        self.file.write_bytes(b"local\n")
        with mock.patch.object(client.opener, "open", side_effect=TimeoutError(TOKEN)) as request:
            self.assertFalse(self.run_sync(client)[0])
            self.assertEqual(request.call_count, 1)

    def test_api_smoke_script_and_persistence_check_are_runnable(self):
        env = dict(os.environ, WIKIKIKI_BASE=self.base, WIKIKIKI_TOKEN=TOKEN)
        command = [sys.executable, "-B", str(SCRIPT.parent / "tests/smoke_http.py"),
                   "--path", "tests/smoke-helper", "--expect-private"]
        for extra in ([], ["--verify-only"]):
            result = subprocess.run(command + extra, env=env, capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn(TOKEN, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
