"""Loopback-only sparse overlay for distribution_registry.rs (Python 3.9 stdlib).

Generated names never fall through to crates.io. Foreign indices are fetched
verbatim, and foreign archives are redirected to the official download host.
No upload route exists. The Rust child guard terminates this server on every
success/error/panic; binding port zero avoids races between concurrent runs.
"""
import hashlib
import http.server
import json
import pathlib
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request

ROOT = pathlib.Path(sys.argv[1]).resolve()
LOG = pathlib.Path(sys.argv[2])
NAMES = set(json.loads((ROOT / "names.json").read_text()))
LOCK = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def record(self, status, origin, data, location):
        with LOCK:
            with LOG.open("a") as stream:
                stream.write(json.dumps({"method": self.command, "path": self.path,
                                         "status": status, "origin": origin,
                                         "response_sha256": hashlib.sha256(data).hexdigest(),
                                         "response_bytes": len(data),
                                         "redirect": location}) + "\n")

    def reply(self, status, data=b"", origin="overlay", location=None):
        self.record(status, origin, data, location)
        self.send_response(status)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-cache")
        if location:
            self.send_header("Location", location)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urllib.parse.urlsplit(self.path).path
        parts = path.strip("/").split("/")
        if any(part in (".", "..") for part in parts):
            return self.reply(400)
        if path == "/index/config.json":
            base = "http://127.0.0.1:{}".format(self.server.server_address[1])
            return self.reply(200, json.dumps({
                "dl": base + "/archives/{crate}/{version}/download", "api": base
            }).encode())
        if parts[0] == "index" and len(parts) >= 3:
            local = ROOT.joinpath(*parts)
            if local.is_file():
                return self.reply(200, local.read_bytes())
            if parts[-1] in NAMES:
                return self.reply(404, origin="unregistered-generated-name")
            url = "https://index.crates.io/" + "/".join(parts[1:])
            try:
                with urllib.request.urlopen(url, timeout=120) as response:
                    return self.reply(response.status, response.read(), origin=url)
            except urllib.error.HTTPError as error:
                return self.reply(error.code, error.read(), origin=url)
            except (urllib.error.URLError, TimeoutError) as error:
                return self.reply(502, str(error).encode(), origin=url)
        if len(parts) == 4 and parts[0] == "archives" and parts[3] == "download":
            name, version = parts[1:3]
            local = ROOT.joinpath(*parts)
            if name in NAMES:
                if local.is_file():
                    return self.reply(200, local.read_bytes())
                return self.reply(404, origin="unregistered-generated-archive")
            url = "https://static.crates.io/crates/{0}/{0}-{1}.crate".format(name, version)
            return self.reply(302, origin=url, location=url)
        self.reply(404)

    def do_PUT(self):
        self.reply(405, b"Uploads are forbidden by this proof")

    def do_POST(self):
        self.reply(405, b"Uploads are forbidden by this proof")


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
server.timeout = 1
print(server.server_address[1], flush=True)
server.serve_forever()
