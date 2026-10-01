"""Local pages for the probe scripts (key-probe.sh, page-kind-probe.sh).

Usage: python -P scripts/probe_server.py <port-file> <log-file> <seconds> < /dev/null

GET / is a page that reports every keydown it sees: its handler requests
/k?code=..&key=..&kc=.., and this server appends `code=<..> key=<..> kc=<..>`
to <log-file>. GET /data.json is a JSON response and GET /pixel.png a small
valid PNG, each with its real content type, for the page-kind probe. Writes
the port to <port-file> and exits after <seconds> on its own.
"""

import http.server
import struct
import sys
import threading
import time
import urllib.parse
import zlib

port_file, log_file, lifetime = sys.argv[1], sys.argv[2], float(sys.argv[3])
JSON = b'{"probe": [1, 2, 3]}'


def make_png(width=4, height=4):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    rows = b"".join(b"\x00" + b"\xd7\xa2\x4a" * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


PNG = make_png()
PAGE = b"""<!doctype html><title>keys</title><textarea id=t autofocus></textarea>
<script>
document.getElementById('t').focus();
addEventListener('keydown', e => {
  const q = new URLSearchParams({code: e.code, key: e.key, kc: e.keyCode});
  new Image().src = '/k?' + q;
});
</script>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        if url.path == "/k":
            q = urllib.parse.parse_qs(url.query, keep_blank_values=True)
            with open(log_file, "a", newline="\n") as f:
                f.write("code=%s key=%s kc=%s\n" % (q["code"][0], q["key"][0], q["kc"][0]))
            self.send_response(204)
            self.end_headers()
            return
        ctype, body = {"/data.json": ("application/json", JSON), "/pixel.png": ("image/png", PNG)}.get(
            url.path, ("text/html", PAGE)
        )
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
threading.Thread(target=server.serve_forever, daemon=True).start()
time.sleep(lifetime)
server.shutdown()
