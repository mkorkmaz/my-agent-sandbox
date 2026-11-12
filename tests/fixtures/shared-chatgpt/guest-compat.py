"""Exercise the installed guest CLI against a loopback-only fake Responses server.

All credentials are synthetic. This never contacts OpenAI or reads a user's home.
"""
import base64
import http.server
import json
import os
from pathlib import Path
import subprocess
import ssl
import tempfile
import threading
import time


def jwt(claims):
    encode = lambda value: base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip('=')
    return encode({'alg': 'none'}) + '.' + encode(claims) + '.synthetic-signature'


def auth(account, access):
    identity = jwt({'email': 'fixture@example.invalid', 'https://api.openai.com/auth': {
        'chatgpt_account_id': account, 'chatgpt_plan_type': 'plus', 'chatgpt_user_id': 'fixture-user'}})
    return {'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None, 'tokens': {
        'id_token': identity, 'access_token': access, 'refresh_token': '', 'account_id': account},
        'last_refresh': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}


def run_case(root, case, mismatch=False):
    directory = root / case
    directory.mkdir()
    old = jwt({'exp': int(time.time()) + 3600, 'fixture': 'before'})
    new = jwt({'exp': int(time.time()) + 7200, 'fixture': 'after'})
    auth_file = directory / 'auth.json'
    auth_file.write_text(json.dumps(auth('fixture-account', old)))
    seen = []
    paths = []
    blocked_external = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            paths.append(self.path)
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            data = {'models': []}
            if self.path.endswith('/accounts/check'):
                data = {'accounts': [{'id': 'fixture-account', 'workspace_backend_origin': 'NO_CONSTRAINT',
                                      'account_routing_override': 'NO_CONSTRAINT'}],
                        'account_ordering': ['fixture-account'], 'default_account_id': 'fixture-account'}
            self.wfile.write(json.dumps(data).encode())

        def do_POST(self):
            paths.append(self.path)
            self.rfile.read(int(self.headers.get('Content-Length', '0')))
            if not self.path.endswith('/responses'):
                self.send_response(404)
                self.end_headers()
                return
            token = self.headers.get('Authorization', '')
            seen.append('before' if token == 'Bearer ' + old else 'after' if token == 'Bearer ' + new else 'unexpected')
            if len(seen) == 1 and case != 'access-only':
                replacement = auth('different-account' if mismatch else 'fixture-account', new)
                temporary = directory / 'updated.json'
                temporary.write_text(json.dumps(replacement))
                temporary.replace(auth_file)
                self.send_response(401)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(b'{"error":{"message":"synthetic expired access token","type":"invalid_request_error"}}')
                return
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Connection', 'close')
            self.end_headers()
            message = {'id': 'message-fixture', 'type': 'message', 'role': 'assistant', 'content': [
                {'type': 'output_text', 'text': 'shared-auth-fixture-ok'}]}
            events = [
                {'type': 'response.created', 'response': {'id': 'response-fixture'}},
                {'type': 'response.output_item.done', 'output_index': 0, 'item': message},
                {'type': 'response.completed', 'response': {'id': 'response-fixture', 'status': 'completed',
                    'output': [message], 'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}},
            ]
            for event in events:
                self.wfile.write(('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n').encode())

    class DenyProxy(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_CONNECT(self):
            blocked_external.append(self.path)
            self.send_response(403)
            self.end_headers()

    proxy = http.server.ThreadingHTTPServer(('127.0.0.1', 0), DenyProxy)
    threading.Thread(target=proxy.serve_forever, daemon=True).start()
    certificate = root / 'test-ca.pem'
    key = root / 'test-key.pem'
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(root / 'test-server.pem', key)
    server.socket = tls.wrap_socket(server.socket, server_side=True)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    endpoint = 'https://127.0.0.1:' + str(server.server_address[1])
    environment = {'HOME': str(root), 'CODEX_HOME': str(directory), 'PATH': os.environ['PATH'],
                   'TERM': 'dumb', 'LANG': 'C.UTF-8', 'DO_NOT_TRACK': '1',
                   'CODEX_CA_CERTIFICATE': str(certificate), 'SSL_CERT_FILE': str(certificate),
                   'HTTPS_PROXY': 'http://127.0.0.1:' + str(proxy.server_address[1]),
                   'HTTP_PROXY': 'http://127.0.0.1:' + str(proxy.server_address[1]),
                   'NO_PROXY': '127.0.0.1,localhost'}
    command = ['codex', '--no-daemon', '-c', 'cli_auth_credentials_store="file"',
               '-c', 'chatgpt_base_url=' + json.dumps(endpoint),
               '-c', 'model_provider="fixture"',
               '-c', 'model_providers.fixture.name="Synthetic local fixture"',
               '-c', 'model_providers.fixture.base_url=' + json.dumps(endpoint + '/codex'),
               '-c', 'model_providers.fixture.wire_api="responses"',
               '-c', 'model_providers.fixture.requires_openai_auth=true',
               '-c', 'model_providers.fixture.supports_websockets=false',
               '-c', 'model_providers.fixture.request_max_retries=0',
               '-c', 'model_providers.fixture.stream_max_retries=0',
               '-c', 'features.apps=false', 'exec', '--json', '--skip-git-repo-check',
               '--sandbox', 'read-only', '--model', 'gpt-6.1-sol', 'Reply with the fixture response.']
    try:
        try:
            result = subprocess.run(command, env=environment, cwd=root, stdin=subprocess.DEVNULL,
                                    capture_output=True, text=True, timeout=40)
        except subprocess.TimeoutExpired as error:
            return {'case': case, 'passed': False, 'exitCode': None, 'requests': seen, 'paths': paths,
                    'blockedExternalHosts': sorted(set(blocked_external)),
                    'output': (error.stdout or b'').decode(errors='replace')[-6000:],
                    'error': 'Timed out. ' + (error.stderr or b'').decode(errors='replace')[-6000:]}
        success = result.returncode == 0 and 'shared-auth-fixture-ok' in result.stdout
        passed = success and seen == (['before'] if case == 'access-only' else ['before', 'after'])
        if mismatch:
            passed = not success and seen == ['before']
        return {'case': case, 'passed': passed, 'exitCode': result.returncode, 'requests': seen,
                'paths': paths, 'blockedExternalHosts': sorted(set(blocked_external)),
                'output': result.stdout[-6000:], 'error': result.stderr[-6000:]}
    finally:
        server.shutdown()
        server.server_close()
        proxy.shutdown()
        proxy.server_close()


with tempfile.TemporaryDirectory(prefix='tugboat-shared-auth-') as temporary:
    root = Path(temporary)
    config = root / 'openssl.cnf'
    config.write_text('[req]\ndistinguished_name=dn\nx509_extensions=extensions\nprompt=no\n'
                      '[dn]\nCN=Tugboat synthetic test CA\n[extensions]\n'
                      'basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,digitalSignature,keyEncipherment\n'
                      '[server]\nbasicConstraints=critical,CA:FALSE\n'
                      'keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n'
                      'subjectAltName=IP:127.0.0.1,DNS:localhost\n')
    subprocess.run(['openssl', 'req', '-new', '-x509', '-nodes', '-newkey', 'rsa:2048',
                    '-keyout', str(root / 'test-ca-key.pem'), '-out', str(root / 'test-ca.pem'),
                    '-days', '1', '-config', str(config)], check=True, capture_output=True)
    subprocess.run(['openssl', 'req', '-new', '-nodes', '-newkey', 'rsa:2048',
                    '-keyout', str(root / 'test-key.pem'), '-out', str(root / 'test-server.csr'),
                    '-subj', '/CN=localhost'], check=True, capture_output=True)
    subprocess.run(['openssl', 'x509', '-req', '-in', str(root / 'test-server.csr'),
                    '-CA', str(root / 'test-ca.pem'), '-CAkey', str(root / 'test-ca-key.pem'),
                    '-CAcreateserial', '-out', str(root / 'test-server.pem'), '-days', '1',
                    '-extfile', str(config), '-extensions', 'server'], check=True, capture_output=True)
    report = {'schemaVersion': 1, 'syntheticCredentials': True, 'externalRequests': False,
              'version': subprocess.run(['codex', '--version'], capture_output=True, text=True).stdout.strip(),
              'cases': [run_case(root, 'access-only'), run_case(root, 'reload-after-401'),
                        run_case(root, 'reject-account-change', mismatch=True)]}
    print(json.dumps(report, ensure_ascii=False, indent=2))
    raise SystemExit(0 if all(case['passed'] for case in report['cases']) else 1)
