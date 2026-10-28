#!/usr/bin/env python3
"""Force real TURN UDP/TCP/TLS, Noise, RTP video and negative TLS cases locally.
Requires coturn, openssl and cargo. Never installs a root certificate globally.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time

root = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--turnserver', default=shutil.which('turnserver'))
p.add_argument('--cargo', default='cargo')
a = p.parse_args()
if not a.turnserver:
    p.error('Install coturn first (brew install coturn / apt install coturn).')

def port():
    with socket.socket() as server:
        server.bind(('127.0.0.1', 0))
        return server.getsockname()[1]

def run(*args, **kwargs):
    subprocess.run(args, check=True, **kwargs)

with tempfile.TemporaryDirectory(prefix='dogcodes-turn-') as directory, tempfile.TemporaryFile() as log:
    folder = Path(directory)
    def openssl(*args):
        run('openssl', *args, cwd=folder, stdout=log, stderr=log)
    openssl('req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-days','1',
        '-subj','/CN=DogCodes isolated TURN CA','-addext','basicConstraints=critical,CA:TRUE',
        '-keyout','ca.key','-out','ca.pem')
    openssl('req','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-subj','/CN=localhost',
        '-keyout','server.key','-out','server.csr')
    (folder/'extensions').write_text('subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n')
    openssl('x509','-req','-in','server.csr','-CA','ca.pem','-CAkey','ca.key','-CAcreateserial',
        '-days','1','-extfile','extensions','-out','server.pem')
    udp, tls = port(), port()
    username, password = 'isolated-test', secrets.token_hex(24)
    config = folder/'turn.conf'
    config.write_text(f'''listening-ip=127.0.0.1
relay-ip=127.0.0.1
listening-port={udp}
tls-listening-port={tls}
min-port=49160
max-port=49260
realm=dogcodes-test.invalid
userdb={folder/'users.sqlite'}
relay-threads=1
lt-cred-mech
user={username}:{password}
cert={folder/'server.pem'}
pkey={folder/'server.key'}
no-cli
no-multicast-peers
allow-loopback-peers
pidfile={folder/'turn.pid'}
log-file=stdout
simple-log
verbose
''')
    config.chmod(0o600)
    for file in folder.glob('*.key'):
        file.chmod(0o600)
    server = subprocess.Popen([a.turnserver,'-c',str(config),'-l',str(folder/'turn.log')], stdout=log, stderr=log)
    proxy = socket.socket()
    proxy.bind(('127.0.0.1',0))
    proxy.listen()
    proxy.settimeout(.2)
    proxy_port = proxy.getsockname()[1]
    stop = threading.Event()
    connections = []
    def pump(source, target):
        try:
            while not stop.is_set():
                chunk = source.recv(65536)
                if not chunk: break
                # Force partial four-byte TURN headers and split large bodies.
                for start,end in [(0,1),(1,3),(3,17),(17,len(chunk))]:
                    if start < len(chunk): target.sendall(chunk[start:min(end,len(chunk))])
        except OSError:
            pass
        finally:
            for sock in (source,target):
                try: sock.shutdown(socket.SHUT_RDWR)
                except OSError: pass
    def accept_proxy():
        while not stop.is_set():
            try:
                incoming,_ = proxy.accept()
                upstream = socket.create_connection(('127.0.0.1',udp),timeout=3)
                upstream.settimeout(None)
                incoming.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
                upstream.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
                connections.extend([incoming,upstream])
                threading.Thread(target=pump,args=(incoming,upstream),daemon=True).start()
                threading.Thread(target=pump,args=(upstream,incoming),daemon=True).start()
            except socket.timeout: pass
            except OSError: break
    threading.Thread(target=accept_proxy,daemon=True).start()
    try:
        for _ in range(100):
            if server.poll() is not None:
                raise RuntimeError('coturn fixture failed to start')
            try:
                with socket.create_connection(('127.0.0.1',udp),timeout=.1):
                    break
            except OSError:
                time.sleep(.05)
        else:
            raise RuntimeError('coturn fixture startup timed out')
        env = dict(os.environ, SSL_CERT_FILE=str(folder/'ca.pem'))
        # Build once before starting the individual bounded acceptance cases.
        run(a.cargo,'test','-p','dogcodes-remote-transport','--lib','--locked','--no-run',cwd=root,env=env)
        cases = [
            ('UDP',f'turn:127.0.0.1:{udp}?transport=udp',False,True),
            ('TCP',f'turn:127.0.0.1:{udp}?transport=tcp',False,True),
            ('TLS',f'turns:localhost:{tls}?transport=tcp',False,True),
            ('TCP fragmented stream',f'turn:127.0.0.1:{proxy_port}?transport=tcp',False,True),
            ('TLS rejects hostname mismatch',f'turns:127.0.0.1:{tls}?transport=tcp',True,True),
            ('TLS rejects untrusted certificate',f'turns:localhost:{tls}?transport=tcp',True,False),
            ('TCP rejects unavailable server',f'turn:127.0.0.1:{port()}?transport=tcp',True,True),
        ]
        for name,url,reject,trust in cases:
            data = folder/'case.json'
            data.write_text(json.dumps({'ice':{'urls':[url],'username':username,'credential':password},'reject':reject}))
            data.chmod(0o600)
            case_env = dict(env, DOGCODES_TURN_TEST_CONFIG=str(data))
            if not trust:
                # Explicit empty trust file; no private CA is installed in OS trust.
                (folder/'empty-roots.pem').touch()
                case_env['SSL_CERT_FILE'] = str(folder/'empty-roots.pem')
            print(f'TURN acceptance: {name}',flush=True)
            run(a.cargo,'test','-p','dogcodes-remote-transport','--lib','--locked',
                'turn_acceptance::turn_acceptance','--','--ignored','--exact','--nocapture',cwd=root,env=case_env,timeout=75)
            if not reject:
                print(f'TURN video acceptance: {name}',flush=True)
                run(a.cargo,'test','-p','dogcodes-remote-transport','--lib','--locked',
                    'turn_acceptance::turn_video_acceptance','--','--ignored','--exact','--nocapture',cwd=root,env=case_env,timeout=75)
        print('PASS: forced UDP/TCP/TLS relay, fresh pinned Noise, ordered encrypted data, RTP H.264 with concurrent control and PLI, TLS trust/name validation and unavailable-server rejection.')
    except Exception:
        log.seek(0)
        print(log.read().decode(errors='replace').replace(password, '<redacted>')[-5000:])
        raise
    finally:
        stop.set()
        proxy.close()
        for connection in connections:
            connection.close()
        if server.poll() is None:
            server.send_signal(signal.SIGTERM)
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
