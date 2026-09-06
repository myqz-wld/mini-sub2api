"""Pin official OpenCode artifacts locally; never install a global executable."""
import base64
import hashlib
import json
import pathlib
import platform
import tarfile
import urllib.request

version = '1.18.29'
root = pathlib.Path(__file__).resolve().parent.parent
dest = root / 'build/third-party/opencode'
dest.mkdir(parents=True, exist_ok=True)

def fetch_json(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        return json.load(response)

def download(url, target):
    with urllib.request.urlopen(url, timeout=30) as response, target.open('wb') as stream:
        while chunk := response.read(1024 * 1024):
            stream.write(chunk)
    return hashlib.sha256(target.read_bytes()).hexdigest()

system = platform.system().lower()
architecture = {'arm64': 'arm64', 'aarch64': 'arm64', 'x86_64': 'x64'}.get(platform.machine().lower())
if system not in ('darwin', 'linux') or architecture is None:
    raise SystemExit('Scaffold preparation supports macOS/Linux x64 or arm64.')
package = 'opencode-' + system + '-' + architecture
if architecture == 'x64':
    package += '-baseline'
if system == 'linux' and platform.libc_ver()[0] == 'musl':
    package += '-musl'
metadata = fetch_json('https://registry.npmjs.org/' + package + '/' + version)
archive = dest / 'platform.tgz'
platform_hash = download(metadata['dist']['tarball'], archive)
algorithm, digest = metadata['dist']['integrity'].split('-', 1)
if algorithm != 'sha512' or base64.b64encode(hashlib.sha512(archive.read_bytes()).digest()).decode() != digest:
    raise SystemExit('OpenCode platform integrity verification failed.')
with tarfile.open(archive) as source:
    source.extractall(dest / 'platform', filter='data')
print('OpenCode platform artifact downloaded and npm integrity verified.', flush=True)
tag = fetch_json('https://api.github.com/repos/anomalyco/opencode/git/ref/tags/v' + version)
obj = tag['object']
if obj['type'] == 'tag':
    obj = fetch_json(obj['url'])['object']
if obj['type'] != 'commit':
    raise SystemExit('OpenCode source tag is not a commit.')
commit = obj['sha']
if commit != '16747470f976aca3d362ad730bcd3fe82ecc2c9a':
    raise SystemExit('OpenCode source tag moved.')
source_archive = dest / 'source.tgz'
source_hash = download('https://codeload.github.com/anomalyco/opencode/tar.gz/' + commit, source_archive)
with tarfile.open(source_archive) as source:
    source.extractall(dest / 'source', filter='data')
record = {'version': version, 'native_commit': commit, 'platform_package': package, 'platform_sha256': platform_hash,
          'source_archive_sha256': source_hash, 'npm_integrity_verified': True,
          'binary': 'build/third-party/opencode/platform/package/bin/opencode',
          'source': 'build/third-party/opencode/source/opencode-' + commit,
          'global_installation': False}
(dest / 'reference.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record), flush=True)
