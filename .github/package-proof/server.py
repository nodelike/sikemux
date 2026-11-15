# Serves a folder the way GitHub serves releases: /releases/download/<tag>/<asset>
# answers 302 to where the file is, as GitHub's object store does.
import http.server, os, sys

ROOT = sys.argv[1]

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=ROOT, **kwargs)

    def do_GET(self):
        prefix = "/releases/download/"
        if self.path.startswith(prefix):
            target = "/objects/" + self.path[len(prefix):]
            if not os.path.isfile(os.path.join(ROOT, target.lstrip("/"))):
                self.send_error(404)
                return
            self.send_response(302)
            self.send_header("Location", target)
            self.end_headers()
            return
        super().do_GET()

http.server.ThreadingHTTPServer(("127.0.0.1", 8080), Handler).serve_forever()
