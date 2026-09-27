"""Serves one medium page at any path, for scale-test.sh.

Usage: python -P scripts/page_server.py <port-file> <seconds> < /dev/null

The page is ~60 KB of text and a script that retitles it every second, a
light stand-in for a real site. Writes the port to <port-file> and exits
after <seconds> on its own, so nothing has to kill it.
"""

import http.server
import sys
import threading
import time

port_file, lifetime = sys.argv[1], float(sys.argv[2])
PARA = "<p>" + "Lorem ipsum dolor sit amet, consectetur adipiscing elit. " * 20 + "</p>\n"
BODY = (
    "<!doctype html><title>page</title><body>"
    + PARA * 50
    + "<script>let n = 0; setInterval(function () { document.title = 'page ' + (++n); }, 1000);</script>"
).encode()


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
threading.Thread(target=server.serve_forever, daemon=True).start()
time.sleep(lifetime)
server.shutdown()
