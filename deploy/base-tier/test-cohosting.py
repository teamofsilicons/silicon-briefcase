#!/usr/bin/env python3
"""Exercise the actual Nginx cohosting maps in an isolated loopback process."""
import argparse
import pathlib
import re
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--nginx', default=shutil.which('nginx') or '/usr/sbin/nginx')
args = parser.parse_args()
source = pathlib.Path(__file__).with_name('accounts-browser.nginx.conf').read_text()
def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]
port, legacy, accounts = (free_port() for _ in range(3))
maps = source[:source.index('server {')]
server = source[source.index('server {', source.index('server {') + 1):]
server = re.sub(r'^\s*(listen|server_name|ssl_certificate|ssl_certificate_key) .*;\n', '', server, flags=re.M)
server = server.replace('server {', f'server {{\n listen 127.0.0.1:{port};', 1)
config = (maps + server).replace('4318', str(legacy)).replace('4319', str(accounts))
config += f'\nserver {{ listen 127.0.0.1:{legacy}; return 200 "legacy"; }}\nserver {{ listen 127.0.0.1:{accounts}; return 200 "accounts"; }}\n'
cases = [
    ('/', 200, 'accounts'), ('/org/tos/private.txt', 200, 'legacy'),
    ('/org/4eb1de48-df66-4801-ba07-d69f59c81692/apps/extend/image.png', 200, 'accounts'),
    ('/browser/session', 200, 'legacy'), ('/assets/main.js', 200, 'legacy'),
    ('/api/v1/uploads', 200, 'accounts'), ('/browser/upload', 200, 'legacy'),
    ('/auth/callback?code=abc&state=xyz', 200, 'accounts'),
    ('/auth/callback?state=xyz&slt=slt_abc', 200, 'legacy'),
    ('/auth/callback?error=access_denied&error_description=Cancelled&state=xyz', 200, 'accounts'),
    ('/auth/callback?code=abc&slt=slt_abc&state=xyz', 400, None),
    ('/auth/callback?code=abc&slt=&state=xyz', 400, None),
    ('/auth/callback?code=&slt=slt_abc&state=xyz', 400, None),
    ('/auth/callback?code=abc&error=bad&state=xyz', 400, None),
    ('/auth/callback?code=abc&code=def&state=xyz', 400, None),
    ('/auth/callback?code=abc&state=xyz&state=def', 400, None),
    ('/auth/callback?code=abc&%73lt=slt_abc&state=xyz', 400, None),
    ('/auth/callback?Code=abc&state=xyz', 400, None),
    ('/auth/callback?code=abc', 400, None), ('/auth/callback?state=xyz', 400, None),
]
with tempfile.TemporaryDirectory(prefix='briefcase-nginx-proof-') as directory:
    root = pathlib.Path(directory)
    path = root/'nginx.conf'
    path.write_text(f'pid {root}/nginx.pid;\nerror_log {root}/error.log;\nevents {{}}\nhttp {{ access_log off; {config} }}\n')
    subprocess.run([args.nginx, '-p', str(root), '-c', str(path), '-t'], check=True)
    process = subprocess.Popen([args.nginx, '-p', str(root), '-c', str(path), '-g', 'daemon off;'])
    try:
        for _ in range(30):
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=.2): break
            except OSError: time.sleep(.1)
        for route, status, body in cases:
            try: response = urllib.request.urlopen(f'http://127.0.0.1:{port}{route}', timeout=3)
            except urllib.error.HTTPError as error: response = error
            with response:
                actual = response.read().decode()
                assert response.status == status, (route, response.status, status)
                if body is not None: assert actual == body, (route, actual, body)
        print(f'{len(cases)} Nginx cohosting and callback isolation checks passed')
    finally:
        process.terminate()
        process.wait(timeout=5)
