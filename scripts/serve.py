#!/usr/bin/env python3
"""Serve `site/` the way a real host would.

`python3 -m http.server` sends everything uncompressed, which turns a 12 MB
download into a 29 MB one -- most of the difference being the wasm module. This
serves gzip when the client asks for it, compressing once and caching the
result next to the original, which is what GitHub Pages does and what makes a
phone on Wi-Fi behave like the deployed site.
"""

import gzip
import os
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "site")
COMPRESS = (".wasm", ".js", ".otf", ".ttf", ".html", ".json")


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ROOT, **kw)

    def send_head(self):
        path = self.translate_path(self.path)
        accepts = "gzip" in self.headers.get("Accept-Encoding", "")
        if accepts and path.endswith(COMPRESS) and os.path.isfile(path):
            packed = path + ".gz"
            if not os.path.isfile(packed) or os.path.getmtime(packed) < os.path.getmtime(path):
                with open(path, "rb") as src, gzip.open(packed, "wb", compresslevel=6) as dst:
                    dst.write(src.read())
            f = open(packed, "rb")
            self.send_response(200)
            self.send_header("Content-Type", self.guess_type(path))
            self.send_header("Content-Encoding", "gzip")
            self.send_header("Content-Length", str(os.path.getsize(packed)))
            # A phone reloading the page should not fetch it all again.
            self.send_header("Cache-Control", "public, max-age=600")
            self.end_headers()
            return f
        return super().send_head()

    def log_message(self, fmt, *args):
        sys.stderr.write("  %s\n" % (fmt % args))


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8787
    print(f"serving {os.path.realpath(ROOT)} on 0.0.0.0:{port}")
    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()
