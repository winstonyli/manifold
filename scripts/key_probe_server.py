"""Serves a page that reports every keydown it sees, for key-probe.sh.

Usage: python -P scripts/key_probe_server.py <port-file> <log-file> <seconds> < /dev/null

GET / is the page. Its keydown handler requests /k?code=..&key=..&kc=.., and
this server appends `code=<..> key=<..> kc=<..>` to <log-file>. Writes the
port to <port-file> and exits after <seconds> on its own.
"""

import http.server
import sys
import threading
import time
import urllib.parse

port_file, log_file, lifetime = sys.argv[1], sys.argv[2], float(sys.argv[3])
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
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
threading.Thread(target=server.serve_forever, daemon=True).start()
time.sleep(lifetime)
server.shutdown()
