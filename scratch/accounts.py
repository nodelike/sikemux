import json, datetime
from http.server import BaseHTTPRequestHandler, HTTPServer

class Answer(BaseHTTPRequestHandler):
    def reply(self):
        now = datetime.datetime.now(datetime.timezone.utc)
        stamp = lambda at: at.strftime('%Y-%m-%dT%H:%M:%SZ')
        answers = {
            '/v1/host-sign-ins': {'code': 'c' * 64, 'userCode': 'BCDF GHJK', 'verificationUrl': 'http://127.0.0.1:4000/approve', 'expiresAt': stamp(now + datetime.timedelta(minutes=10)), 'interval': 1},
            '/v1/host-sign-ins/poll': {'status': 'approved', 'account': 'user_login', 'nonce': 'b' * 64},
            '/v1/host-sign-ins/finish': {'key': 'k', 'role': 'host', 'name': 'proof', 'platform': 'linux', 'createdAt': stamp(now), 'lastSeenAt': None},
        }
        answer = answers.get(self.path)
        print(self.command, self.path, flush=True)
        body = json.dumps(answer if answer else {}).encode()
        self.send_response(200 if answer else 404)
        self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = reply

HTTPServer(('127.0.0.1', 4000), Answer).serve_forever()
