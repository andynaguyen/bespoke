#!/usr/bin/env python3
"""Build and transfer dependency groups within one cold GitHub Actions run."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def identity():
    return {
        'commit': output('git', 'rev-parse', 'HEAD'),
        'rust': output('rustc', '-vV'),
        'xcode': output('xcodebuild', '-version'),
        'profile': hashlib.sha256(Path('scripts/ci/release-dsp.toml').read_bytes()).hexdigest(),
    }


def prepare():
    # Cargo compares source mtimes with fingerprints. Each fresh checkout must
    # use the same source times before compiling or consuming these artifacts.
    timestamp = int(output('git', 'show', '-s', '--format=%ct', 'HEAD'))
    tracked = subprocess.check_output(['git', 'ls-files', '-z']).split(b'\0')
    for raw in tracked:
        if raw:
            os.utime(os.fsdecode(raw), (timestamp, timestamp), follow_symlinks=False)
    cargo = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    for directory in (cargo / 'registry/src', cargo / 'git/checkouts'):
        if directory.exists():
            for root, dirs, files in os.walk(directory):
                dirs[:] = [d for d in dirs if d != '.git']
                for name in files:
                    os.utime(Path(root) / name, (timestamp, timestamp), follow_symlinks=False)


def build(group):
    host = next(line.split(': ', 1)[1] for line in output('rustc', '-vV').splitlines() if line.startswith('host: '))
    metadata = json.loads(output('cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', host))
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    def dependencies(package):
        return {d['pkg'] for d in nodes[package]['deps'] if any(k['kind'] is None for k in d['dep_kinds'])}
    direct = dependencies(metadata['resolve']['root'])
    by_name = {packages[p]['name']: p for p in direct}
    ui = {by_name[name] for name in ('gpui', 'gpui-component')}
    platform_names = {'arboard', 'auto-launch', 'block2', 'enigo', 'global-hotkey', 'handy-keys',
                      'objc2', 'objc2-app-kit', 'objc2-foundation', 'objc2-service-management',
                      'tray-icon', 'rusqlite', 'rusqlite_migration'}
    platform = {by_name[name] for name in platform_names}
    platform |= dependencies(by_name['scrawl']) - ui
    backend = direct - ui - platform - {by_name['scrawl']}
    selected = {'ui': ui, 'platform': platform, 'backend': backend}[group]
    command = ['cargo', 'build', '--release', '--locked', '--timings', '--config', 'scripts/ci/release-dsp.toml']
    for package in sorted(selected):
        p = packages[package]
        command.extend(['-p', f"{p['name']}@{p['version']}"])
    subprocess.run(command, check=True)


def pack(group, directory):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f'{group}.json').write_text(json.dumps(identity(), sort_keys=True))
    subprocess.run(['tar', '--use-compress-program', 'zstd -1 -T0', '-cf',
                    str(directory / f'{group}.tar.zst'), 'target/release'], check=True)


def restore(directory):
    expected = identity()
    for group in ('ui', 'platform', 'backend'):
        actual = json.loads((directory / f'{group}.json').read_text())
        if actual != expected:
            raise SystemExit(f'{group}: source, compiler, SDK or profile differs')
        subprocess.run(['tar', '--use-compress-program', 'unzstd', '-xf',
                        str(directory / f'{group}.tar.zst')], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'build', 'pack', 'restore'])
    parser.add_argument('--group', choices=['ui', 'platform', 'backend'])
    parser.add_argument('--directory', type=Path)
    args = parser.parse_args()
    if args.action in ('build', 'pack') and not args.group:
        parser.error('--group is required')
    if args.action in ('pack', 'restore') and args.directory is None:
        parser.error('--directory is required')
    if args.action == 'prepare':
        prepare()
    elif args.action == 'build':
        build(args.group)
    elif args.action == 'pack':
        pack(args.group, args.directory)
    else:
        restore(args.directory)


if __name__ == '__main__':
    main()
