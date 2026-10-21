"""Sender TLS must use packaged CAs without requiring host startup hooks."""
import asyncio
import datetime
import ipaddress
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


CLIENT = """
import asyncio, json, sys, urllib.request
sys.path.insert(0, sys.argv[1])
sys.path.insert(0, sys.argv[2])
import aiohttp
import oap_sender
from pyatv.support.http import create_session
from types import SimpleNamespace
from unittest.mock import patch
url = sys.argv[3]
with urllib.request.urlopen(url, timeout=5) as response:
    control = response.status

# Replace only the external device connection; use pyatv's real HTTP session manager and TLS.
async def connect(conf, loop, session=None):
    manager = await create_session(session)
    try:
        async with manager.session.get(url) as response:
            return response.status
    finally:
        await manager.close()

async def no_op():
    pass

async def pair(conf, protocol, loop, session=None):
    await connect(conf, loop, session)
    return SimpleNamespace(begin=no_op, close=no_op)

async def run():
    sidecar = oap_sender.Sidecar()
    sidecar.known['fixture'] = SimpleNamespace(
        services=[], get_service=lambda protocol: SimpleNamespace(requires_password=False))
    try:
        with patch('pyatv.connect', connect), patch('pyatv.pair', pair):
            try:
                if sys.argv[4] == 'pair':
                    await sidecar.pair_begin({'identifier': 'fixture', 'protocol': 'AirPlay'})
                    result = 200
                else:
                    result = await sidecar.open('fixture', {})
            except aiohttp.ClientConnectorCertificateError:
                result = 'certificate_rejected'
        print(json.dumps({'host_control': control, 'sender': result}))
    finally:
        await sidecar.shutdown()
asyncio.run(run())
"""

SERVER = """
import asyncio, ssl, sys
async def serve(reader, writer):
    try:
        await reader.readuntil(b'\\r\\n\\r\\n')
        writer.write(b'HTTP/1.1 200 OK\\r\\nContent-Length: 2\\r\\n\\r\\nOK')
        await writer.drain()
    finally:
        writer.close()
        await writer.wait_closed()
async def main():
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(sys.argv[1], sys.argv[2])
    server = await asyncio.start_server(serve, '127.0.0.1', 0, ssl=context)
    print(server.sockets[0].getsockname()[1], flush=True)
    async with server:
        await server.serve_forever()
asyncio.run(main())
"""


class SenderTLS(unittest.IsolatedAsyncioTestCase):
    async def test_connect_rejects_a_ca_trusted_only_by_the_host(self):
        await self.exercise('connect')

    async def test_pair_rejects_a_ca_trusted_only_by_the_host(self):
        await self.exercise('pair')

    async def exercise(self, mode):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
            name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'TLS test host CA')])
            now = datetime.datetime.now(datetime.timezone.utc)
            cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
                    .public_key(key.public_key()).serial_number(x509.random_serial_number())
                    .not_valid_before(now - datetime.timedelta(days=1))
                    .not_valid_after(now + datetime.timedelta(days=1))
                    .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
                    .add_extension(x509.SubjectAlternativeName([
                        x509.IPAddress(ipaddress.ip_address('127.0.0.1'))]), critical=False)
                    .sign(key, hashes.SHA256()))
            cert_path, key_path = directory / 'ca.pem', directory / 'key.pem'
            cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
            key_path.write_bytes(key.private_bytes(serialization.Encoding.PEM,
                                                  serialization.PrivateFormat.PKCS8,
                                                  serialization.NoEncryption()))
            # Keep the fixture server independent of any host SSL startup hooks too.
            server = await asyncio.create_subprocess_exec(
                sys.executable, '-I', '-S', '-c', SERVER, str(cert_path), str(key_path),
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            try:
                port = (await asyncio.wait_for(server.stdout.readline(), timeout=10)).decode().strip()
                self.assertTrue(port.isdigit(), 'TLS fixture server did not start')
                # Skip .pth startup hooks so a locally installed OS-trust bridge cannot mask this bug.
                site = str(Path(sys.executable).resolve().parents[1] / 'Lib' / 'site-packages')
                if sys.platform != 'win32':
                    site = str(Path(sys.prefix) / 'lib' /
                               f'python{sys.version_info.major}.{sys.version_info.minor}' / 'site-packages')
                proc = await asyncio.create_subprocess_exec(
                    sys.executable, '-I', '-S', '-c', CLIENT, site,
                    str(Path(__file__).resolve().parents[1]), f'https://127.0.0.1:{port}/', mode,
                    env={**os.environ, 'SSL_CERT_FILE': str(cert_path), 'SSL_CERT_DIR': str(directory)},
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                try:
                    stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=20)
                except asyncio.TimeoutError:
                    proc.kill()
                    await proc.communicate()
                    raise
                self.assertEqual(proc.returncode, 0, stderr.decode())
                self.assertEqual(json.loads(stdout), {'host_control': 200, 'sender': 'certificate_rejected'})
            finally:
                if server.returncode is None:
                    server.terminate()
                await server.communicate()
