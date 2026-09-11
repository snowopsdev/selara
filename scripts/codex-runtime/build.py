#!/usr/bin/env python3
"""Build the locked, writing-only variant of the official pinned Codex source."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
VENDOR = ROOT / 'vendor/codex-runtime'
LOCK = tomllib.loads((VENDOR / 'runtime.toml').read_text())

def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

SOURCE_INVENTORY = '.selara-source-inventory.json'

def source_inventory(source):
    files = {}
    for path in sorted(source.rglob('*')):
        relative = path.relative_to(source).as_posix()
        if relative == SOURCE_INVENTORY:
            continue
        if path.is_symlink():
            files[relative] = {'link': os.readlink(path)}
        elif path.is_file():
            files[relative] = {'sha256': digest(path)}
    return files

def source_complete(source):
    # Cargo cache cleanup can preserve a source directory while pruning its
    # files. Check every prepared input, not just directory/marker existence.
    try:
        if source.is_symlink() or not (source / 'codex-rs/Cargo.toml').is_file():
            return False
        expected = json.loads((source / SOURCE_INVENTORY).read_text())
        return expected == source_inventory(source)
    except (OSError, ValueError):
        return False

def prepare_source(source, archive, patches):
    if source_complete(source):
        return
    # The caller holds the build lock and verifies the archive/patch digests.
    # Prepare completely before discarding an incomplete cached source tree.
    with tempfile.TemporaryDirectory(dir=source.parent) as temporary:
        stage = Path(temporary)
        with tarfile.open(archive) as tar:
            for member in tar.getmembers():
                path = Path(member.name)
                if path.is_absolute() or '..' in path.parts:
                    raise SystemExit('unsafe source archive path')
                if member.issym() or member.islnk():
                    destination = path.parent / member.linkname
                    if Path(member.linkname).is_absolute() or '..' in destination.parts:
                        raise SystemExit('unsafe source archive link')
            tar.extractall(stage, filter='data')
        unpacked, = stage.iterdir()
        for patch in patches:
            subprocess.run(['patch', '-p1', '--batch', '--forward', '-i', str(patch)], cwd=unpacked, check=True)
        (unpacked / SOURCE_INVENTORY).write_text(json.dumps(source_inventory(unpacked), sort_keys=True))
        if source.is_symlink() or source.is_file():
            source.unlink()
        elif source.exists():
            shutil.rmtree(source)
        unpacked.rename(source)

def main():
    if os.uname().sysname != 'Darwin' or os.uname().machine != 'arm64':
        raise SystemExit('This runtime lock targets macOS ARM64 only')
    patches = sorted((VENDOR / 'patches').glob('*.patch'))
    patch_digest = hashlib.sha256(b''.join(p.name.encode() + b'\0' + p.read_bytes() for p in patches)).hexdigest()
    if patch_digest != LOCK['patches_sha256']:
        raise SystemExit('runtime patch digest mismatch; review and update runtime.toml')
    cache = ROOT / 'target/codex-runtime'
    cache.mkdir(parents=True, exist_ok=True)
    # Coordinate source preparation and output publication between release jobs.
    with (cache / '.build.lock').open('a+b') as guard:
        fcntl.flock(guard, fcntl.LOCK_EX)
        archive = cache / (LOCK['source_revision'] + '.tar.gz')
        if not archive.exists():
            supplied = os.environ.get('SOURCE_ARCHIVE')
            with tempfile.NamedTemporaryFile(dir=cache, delete=False) as temp:
                temp_path = Path(temp.name)
                if supplied:
                    with open(supplied, 'rb') as source:
                        shutil.copyfileobj(source, temp)
                else:
                    with urllib.request.urlopen(LOCK['source_url']) as source:
                        shutil.copyfileobj(source, temp)
            if digest(temp_path) != LOCK['source_archive_sha256']:
                temp_path.unlink()
                raise SystemExit('official Codex source archive digest mismatch')
            temp_path.replace(archive)
        if digest(archive) != LOCK['source_archive_sha256']:
            raise SystemExit('cached official Codex source archive digest mismatch')
        source = cache / ('source-' + patch_digest)
        prepare_source(source, archive, patches)
        env = dict(os.environ)
        for key in ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTUP_TOOLCHAIN']:
            env.pop(key, None)
        env.update(RUSTUP_TOOLCHAIN=LOCK['rust_toolchain'],
                   MACOSX_DEPLOYMENT_TARGET=LOCK['minimum_macos'],
                   CARGO_TARGET_DIR=str(cache / 'cargo'), CARGO_INCREMENTAL='0',
                   RUSTFLAGS=f'--remap-path-prefix={cache}=codex-src')
        subprocess.run(['cargo', 'build', '--manifest-path', str(source / 'codex-rs/Cargo.toml'), '--locked', '--release', '--target', LOCK['target'], '-p', 'selara-codex'], env=env, check=True)
        built = cache / 'cargo' / LOCK['target'] / 'release/selara-codex'
        destination = ROOT / 'target/selara-codex'
        shutil.copy2(built, destination)
        for name in ['LICENSE', 'NOTICE']:
            shutil.copy2(source / name, ROOT / ('target/selara-codex.' + name))
        provenance = dict(LOCK, artifact_sha256=digest(destination))
        (ROOT / 'target/selara-codex.provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
        print(f'{digest(destination)}  {destination}')

if __name__ == '__main__':
    main()
