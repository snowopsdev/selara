#!/usr/bin/env python3
"""Check, prepare, and finish an update of the pinned official Codex source."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib
import urllib.request

spec = importlib.util.spec_from_file_location('runtime_build', Path(__file__).with_name('build.py'))
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)

ROOT = build.ROOT
VENDOR = build.VENDOR
CACHE = ROOT / 'target/codex-runtime'
REPOSITORY = 'https://github.com/openai/codex'
LATEST_API = 'https://api.github.com/repos/openai/codex/releases/latest'
PATCH_NAME = '0001-selara-writing-runtime.patch'
TAG = re.compile(r'rust-v(\d+)\.(\d+)\.(\d+)')
GIT_ID = ['-c', 'user.name=selara-bump', '-c', 'user.email=selara-bump@invalid']


def tag_version(tag):
    # fullmatch rejects prereleases and trailing text; upstream tags are untrusted.
    match = TAG.fullmatch(tag)
    if not match:
        raise ValueError(f'not a stable Codex release tag: {tag!r}')
    return tuple(int(part) for part in match.groups())


def status(current, tag):
    latest = tag_version(tag)
    return {'current': current, 'latest': '.'.join(map(str, latest)), 'tag': tag,
            'behind': tag_version('rust-v' + current) < latest, 'patches': {}}


def latest_release_tag(token=None):
    headers = {'Accept': 'application/vnd.github+json'}
    if token:
        headers['Authorization'] = f'Bearer {token}'
    with urllib.request.urlopen(urllib.request.Request(LATEST_API, headers=headers), timeout=30) as response:
        return json.load(response)['tag_name']


def peeled_revision(tag, ls_remote):
    refs = dict(reversed(line.split('\t')) for line in ls_remote.splitlines() if '\t' in line)
    revision = refs.get(f'refs/tags/{tag}^{{}}') or refs.get(f'refs/tags/{tag}')
    if not revision:
        raise ValueError(f'{tag} was not found in {REPOSITORY}')
    return revision


def resolve_revision(tag):
    output = subprocess.run(['git', 'ls-remote', REPOSITORY, f'refs/tags/{tag}', f'refs/tags/{tag}^{{}}'],
                            check=True, capture_output=True, text=True).stdout
    return peeled_revision(tag, output)


def download(revision):
    CACHE.mkdir(parents=True, exist_ok=True)
    archive = CACHE / f'{revision}.tar.gz'
    if not archive.exists():
        url = f'https://codeload.github.com/openai/codex/tar.gz/{revision}'
        with tempfile.NamedTemporaryFile(dir=CACHE, delete=False) as temp, urllib.request.urlopen(url, timeout=300) as source:
            shutil.copyfileobj(source, temp)
        Path(temp.name).replace(archive)
    return archive, build.digest(archive)


def patch_files():
    return sorted((VENDOR / 'patches').glob('*.patch'))


def apply_patches(tree, patches):
    # Each patch is 'upstream' (already contained), 'clean', or its rejected files.
    results = {}
    for patch in patches:
        args = ['patch', '-p1', '--batch', '-i', str(patch)]
        if subprocess.run(args + ['--reverse', '--force', '--dry-run'], cwd=tree, capture_output=True).returncode == 0:
            results[patch.name] = 'upstream'
            continue
        before = set(tree.rglob('*.rej'))
        applied = subprocess.run(args + ['--forward'], cwd=tree, capture_output=True, text=True)
        if applied.returncode > 1:
            raise SystemExit(f'{patch.name} could not be applied:\n{applied.stdout}{applied.stderr}')
        rejected = sorted(str(p.relative_to(tree))[:-len('.rej')] for p in set(tree.rglob('*.rej')) - before)
        results[patch.name] = rejected or 'clean'
    return results


def lock_packages(text):
    packages = set()
    for block in text.split('[[package]]')[1:]:
        fields = dict(re.findall(r'^(name|version|source) = "([^"]*)"$', block, re.M))
        packages.add((fields['name'], fields['version'], fields.get('source')))
    return packages


def registry_changes(before, after):
    old = {p for p in lock_packages(before) if p[2]}
    new = {p for p in lock_packages(after) if p[2]}
    return {'removed': sorted(old - new), 'added': sorted(new - old)}


def stale_workspace_packages(text):
    return sorted(name for name, version, source in lock_packages(text) if source is None and version == '0.0.0')


def update_lock_text(text, values):
    for key, value in values.items():
        text, count = re.subn(rf'^{key} = "[^"]*"$', f'{key} = "{value}"', text, flags=re.M)
        if count != 1:
            raise ValueError(f'runtime.toml must contain exactly one {key}')
    return text


def write_patch_set(directory, diff):
    for old in directory.glob('*.patch'):
        old.unlink()
    (directory / PATCH_NAME).write_text(diff)
    return sorted(directory.glob('*.patch'))


def ensure_fresh(target, force):
    if target.exists():
        if not force:
            raise SystemExit(f'{target} already exists and may hold resolved conflicts; pass --force to discard it')
        shutil.rmtree(target)


def git(tree, *args):
    return subprocess.run(['git', *GIT_ID, *args], cwd=tree, check=True, capture_output=True, text=True).stdout


def check(token=None):
    lock = tomllib.loads((VENDOR / 'runtime.toml').read_text())
    result = status(lock['source_version'], latest_release_tag(token))
    if result['behind']:
        revision = resolve_revision(result['tag'])
        archive, sha256 = download(revision)
        with tempfile.TemporaryDirectory() as temporary:
            tree = build.extract_archive(archive, Path(temporary))
            result['patches'] = apply_patches(tree, patch_files())
        result.update(revision=revision, sha256=sha256)
    return result


def prepare(version, force=False):
    tag = f'rust-v{version}'
    tag_version(tag)
    revision = resolve_revision(tag)
    archive, sha256 = download(revision)
    target = CACHE / f'rebase-{version}'
    ensure_fresh(target, force)
    target.mkdir(parents=True)
    tree = build.extract_archive(archive, target)
    git(tree, 'init', '-q')
    git(tree, 'add', '-A', '-f')
    git(tree, 'commit', '-q', '-m', f'upstream {tag}')
    results = apply_patches(tree, patch_files())
    (CACHE / f'rebase-{version}.json').write_text(json.dumps(
        {'version': version, 'tag': tag, 'revision': revision, 'sha256': sha256, 'tree': str(tree)}, indent=2))
    print(f'Prepared {tag} ({revision}) in {tree}')
    for name, state in results.items():
        print(f'  {name}: {state if isinstance(state, str) else "rejected in " + ", ".join(state)}')
    print(f'Resolve every *.rej under {tree}, delete it, then run: bump.py finish {version}')


def finish(version):
    meta = json.loads((CACHE / f'rebase-{version}.json').read_text())
    tree = Path(meta['tree'])
    rejects = sorted(str(p.relative_to(tree)) for p in tree.rglob('*.rej'))
    if rejects:
        raise SystemExit('Resolve and delete these rejects first:\n  ' + '\n  '.join(rejects))
    for backup in tree.rglob('*.orig'):
        backup.unlink()
    lock_path = tree / 'codex-rs/Cargo.lock'
    upstream_lock = git(tree, 'show', 'HEAD:codex-rs/Cargo.lock')
    # Cargo updates only workspace entries here; registry pins must survive untouched.
    subprocess.run(['cargo', 'metadata', '--format-version', '1', '--manifest-path', str(tree / 'codex-rs/Cargo.toml')],
                   env=dict(os.environ, RUSTUP_TOOLCHAIN=build.LOCK['rust_toolchain']), check=True, stdout=subprocess.DEVNULL)
    lock_text = lock_path.read_text()
    changes = registry_changes(upstream_lock, lock_text)
    if changes['removed']:
        raise SystemExit(f'Refusing to change upstream third-party pins: {changes["removed"]}')
    stale = stale_workspace_packages(lock_text)
    if stale:
        raise SystemExit(f'Workspace packages still at 0.0.0: {stale}')
    git(tree, 'add', '-A', '-f')
    diff = git(tree, 'diff', '--cached', '--no-color', '--no-ext-diff', 'HEAD')
    patches = write_patch_set(VENDOR / 'patches', diff)
    runtime = VENDOR / 'runtime.toml'
    runtime.write_text(update_lock_text(runtime.read_text(), {
        'source_version': version,
        'source_revision': meta['revision'],
        'source_url': f'https://codeload.github.com/openai/codex/tar.gz/{meta["revision"]}',
        'source_archive_sha256': meta['sha256'],
        'patches_sha256': build.patch_set_digest(patches),
    }))
    notice = VENDOR / 'NOTICE'
    text, count = re.subn(r'rust-v\d+\.\d+\.\d+', meta['tag'], notice.read_text())
    if count != 1:
        raise SystemExit('NOTICE must name exactly one Codex release')
    notice.write_text(text)
    if changes['added']:
        print(f'New third-party packages for selara-codex: {changes["added"]}')
    print('Updated runtime.toml, NOTICE, and patches. Next: scripts/codex-runtime/build.sh and the runtime test suites.')


def validate_issue_status(result):
    tag_version('rust-v' + result['current'])
    latest = tag_version('rust-v' + result['latest'])
    if tag_version(result['tag']) != latest:
        raise ValueError('latest version must match the release tag')


def issue_title(result):
    validate_issue_status(result)
    return f'Update bundled Codex runtime {result["current"]} → {result["latest"]}'


def issue_body(result):
    validate_issue_status(result)
    lines = [
        f'Codex [{result["tag"]}](https://github.com/openai/codex/releases/tag/{result["tag"]}) is available; '
        f'Selara bundles {result["current"]}.',
        '',
        'ChatGPT hides models whose `minimal_client_version` is newer than the runtime version, '
        'so subscription users miss newer models until this pin moves.',
        '',
        '## Patch status',
    ]
    for name, state in result['patches'].items():
        if state == 'upstream':
            lines.append(f'- `{name}`: already upstream, drop it')
        elif state == 'clean':
            lines.append(f'- `{name}`: applies cleanly')
        else:
            lines.append(f'- `{name}`: rejected hunks in ' + ', '.join(f'`{path}`' for path in state))
    lines += ['', '## Update', '', '```sh', f'python3 scripts/codex-runtime/bump.py prepare {result["latest"]}',
              f'python3 scripts/codex-runtime/bump.py finish {result["latest"]}', '```', '',
              'See "Updating Codex" in `vendor/codex-runtime/README.md`.']
    return '\n'.join(lines) + '\n'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('check')
    prepare_parser = commands.add_parser('prepare')
    prepare_parser.add_argument('version')
    prepare_parser.add_argument('--force', action='store_true')
    commands.add_parser('finish').add_argument('version')
    issue_parser = commands.add_parser('issue')
    issue_parser.add_argument('status')
    part = issue_parser.add_mutually_exclusive_group(required=True)
    part.add_argument('--title', action='store_true')
    part.add_argument('--body', action='store_true')
    args = parser.parse_args(argv)
    if args.command == 'check':
        print(json.dumps(check(os.environ.get('GITHUB_TOKEN')), indent=2))
    elif args.command == 'issue':
        result = json.loads(Path(args.status).read_text())
        tag_version(result['tag'])
        print(issue_title(result) if args.title else issue_body(result), end='\n' if args.title else '')
    else:
        tag_version('rust-v' + args.version)
        prepare(args.version, args.force) if args.command == 'prepare' else finish(args.version)


if __name__ == '__main__':
    main()
