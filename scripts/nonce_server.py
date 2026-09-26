"""A site that redirects to a fresh url on every load, for two-instance-test.sh.

Usage: python -P scripts/nonce_server.py <hits-file> <seconds> < /dev/null

Serves http://127.0.0.1:<port>/r. Each `?n=` it issued is served once;
any other request (no `n`, or an `n` already served) redirects to a new
`n`, like a login nonce that is consumed on use. Prints the port, writes
the running count of /r requests to <hits-file>, and exits after <seconds>
on its own, so nothing has to kill it.
"""

import http.server
import sys
import threading
import time
import urllib.parse

hits_path, lifetime = sys.argv[1], float(sys.argv[2])
lock = threading.Lock()
state = {"next": 0, "issued": set(), "served": set(), "hits": 0}


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        if url.path != "/r":
            self.send_response(404)
            self.end_headers()
            return
        n = urllib.parse.parse_qs(url.query).get("n", [None])[0]
        with lock:
            state["hits"] += 1
            try:
                with open(hits_path, "w") as f:
                    f.write(str(state["hits"]))
            except OSError:
                pass  # the test's output dir is gone: it has finished
            if n in state["issued"] and n not in state["served"]:
                state["served"].add(n)
                redirect = None
            else:
                state["next"] += 1
                redirect = f"/r?n={state['next']}"
                state["issued"].add(str(state["next"]))
        if redirect:
            self.send_response(302)
            self.send_header("Location", redirect)
            self.end_headers()
        else:
            body = b"<!doctype html><title>nonce</title><p>served once"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
time.sleep(lifetime)
server.shutdown()
