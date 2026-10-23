#!/usr/bin/env python3
"""Exercise a running Wikikiki API. Creates one page and retains it for inspection."""

import argparse
import os
import re
import sys
import urllib.error
import urllib.request
import uuid

FIRST = b"# API smoke test\r\n\r\n@literal-file-content\r\n\r\n"
FINAL = b"# API smoke test\n\nConditional update verified.\n\n"


class NoRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--path", default="tests/smoke-" + uuid.uuid4().hex)
    parser.add_argument("--verify-only", action="store_true", help="check the final page after restart or restore")
    parser.add_argument("--expect-private", action="store_true")
    args = parser.parse_args()
    base, token = os.environ.get("WIKIKIKI_BASE", "http://127.0.0.1:8090").rstrip("/"), os.environ.get("WIKIKIKI_TOKEN")
    if not token:
        raise RuntimeError("WIKIKIKI_TOKEN is required")
    if not args.path.startswith("tests/") or any(
        not re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}", segment)
        for segment in args.path.split("/")
    ) or len(args.path) > 200:
        raise RuntimeError("--path must be a valid page under tests/")
    opener = urllib.request.build_opener(NoRedirects())

    def request(method, body=None, extra=None, authenticated=True):
        headers = {"Accept": "text/markdown"}
        if authenticated:
            headers["Authorization"] = "Bearer " + token
        if body is not None:
            headers["Content-Type"] = "text/markdown"
        headers.update(extra or {})
        req = urllib.request.Request(base + "/api/pages/" + args.path, data=body, headers=headers, method=method)
        try:
            response = opener.open(req, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.code, response.headers, response.read()

    def check(condition, message):
        if not condition:
            raise RuntimeError(message)

    if args.verify_only:
        code, _, body = request("GET")
        check(code == 200 and body == FINAL, "persisted page verification failed")
    else:
        code, _, _ = request("GET")
        check(code == 404, "smoke page already exists or GET failed, choose a new --path")
        code, headers, _ = request("PUT", FIRST, {"If-None-Match": "*"})
        check(code == 200 and bool(headers.get("ETag")), "conditional create failed")
        code, headers, body = request("GET")
        etag = headers.get("ETag")
        check(code == 200 and body == FIRST and bool(etag), "GET lost bytes or revision")
        code, _, _ = request("PUT", b"must not overwrite", {"If-None-Match": "*"})
        check(code == 412, "create-if-absent did not reject an existing page")
        code, updated, _ = request("PUT", FINAL, {"If-Match": etag})
        check(code == 200 and updated.get("ETag") != etag, "conditional update failed")
        code, _, _ = request("PUT", b"stale write", {"If-Match": etag})
        check(code == 412, "stale revision did not return 412")
        code, _, body = request("GET")
        check(code == 200 and body == FINAL, "conflict changed the page")
    if args.expect_private:
        code, _, _ = request("GET", authenticated=False)
        check(code == 401, "anonymous read was allowed or returned an unexpected status")
    print("API checks passed, retained page: " + args.path)


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print("error: " + str(error), file=sys.stderr)
        sys.exit(1)
    except Exception:
        # Connection exceptions can include request details. Keep tokens out of logs.
        print("error: API request failed, check service, URL and credentials", file=sys.stderr)
        sys.exit(1)
