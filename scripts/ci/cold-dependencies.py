#!/usr/bin/env python3
"""Build and transfer dependency groups within one cold GitHub Actions run."""
import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess
import time


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def identity():
    subprocess.run(["git", "diff", "--quiet", "HEAD"], check=True)
    return {
        "run": os.environ.get("GITHUB_RUN_ID"),
        "attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        'commit': output('git', 'rev-parse', 'HEAD'),
        'rust': output('rustc', '-vV'),
        'xcode': output('xcodebuild', '-version'),
        'profile': hashlib.sha256(Path('scripts/ci/release-dsp.toml').read_bytes()).hexdigest(),
    }


def source_time():
    return int(output('git', 'show', '-s', '--format=%ct', 'HEAD'))


def set_times(directory, timestamp):
    if not directory.exists():
        return
    for root, dirs, files in os.walk(directory):
        dirs[:] = [d for d in dirs if d != '.git']
        for name in dirs + files:
            os.utime(Path(root) / name, (timestamp, timestamp), follow_symlinks=False)
        os.utime(root, (timestamp, timestamp), follow_symlinks=False)


def prepare():
    # Fingerprints and source directories must have consistent times across
    # fresh checkouts. Contents are fixed by the commit and Cargo.lock.
    timestamp = source_time()
    tracked = subprocess.check_output(['git', 'ls-files', '-z']).split(b'\0')
    parents = set()
    for raw in tracked:
        if raw:
            path = Path(os.fsdecode(raw))
            os.utime(path, (timestamp, timestamp), follow_symlinks=False)
            parents.update(path.parents)
    for parent in parents:
        os.utime(parent, (timestamp, timestamp), follow_symlinks=False)
    cargo = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    for directory in (cargo / 'registry/src', cargo / 'git/checkouts'):
        set_times(directory, timestamp)
    if os.environ.get('TRANSCRIBE_DIR'):
        set_times(Path(os.environ['TRANSCRIBE_DIR']), timestamp)


def dependency_groups(metadata):
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}

    def dependencies(package):
        return {d['pkg'] for d in nodes[package]['deps'] if any(k['kind'] is None for k in d['dep_kinds'])}

    def closure(roots):
        seen, pending = set(), list(roots)
        while pending:
            package = pending.pop()
            if package not in seen:
                seen.add(package)
                pending.extend(dependencies(package) - seen)
        return seen

    direct = dependencies(metadata['resolve']['root'])
    by_name = {packages[p]['name']: p for p in direct}
    ui_libraries = {by_name[name] for name in ('gpui', 'gpui-component')}
    platform_names = {'arboard', 'auto-launch', 'block2', 'enigo', 'global-hotkey', 'handy-keys',
                      'objc2', 'objc2-app-kit', 'objc2-foundation', 'objc2-service-management',
                      'tray-icon', 'rusqlite', 'rusqlite_migration'}
    platform = {p for p in direct if packages[p]['name'] in platform_names}
    platform |= dependencies(by_name['scrawl']) - ui_libraries
    backend = direct - ui_libraries - platform - {by_name['scrawl']}
    # Compile the UI libraries in the final job, once all groups are available.
    # The UI worker only needs prerequisites not already covered by other workers.
    ui = set().union(*(dependencies(p) for p in ui_libraries))
    ui -= ui_libraries | closure(platform | backend)
    return {'ui': ui, 'platform': platform, 'backend': backend}


def build(group):
    host = next(line.split(': ', 1)[1] for line in output('rustc', '-vV').splitlines() if line.startswith('host: '))
    metadata = json.loads(output('cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', host))
    packages = {p['id']: p for p in metadata['packages']}
    selected = dependency_groups(metadata)[group]
    if not selected:
        raise SystemExit(f'No dependency packages selected for {group}')
    command = ['cargo', 'build', '--release', '--locked', '--timings', '--config', 'scripts/ci/release-dsp.toml']
    for package in sorted(selected):
        p = packages[package]
        command.extend(['-p', f"{p['name']}@{p['version']}"])
    print('Building: ' + ', '.join(packages[p]['name'] for p in sorted(selected)), flush=True)
    subprocess.run(command, check=True)


def pack(group, directory):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f'{group}.json').write_text(json.dumps(identity(), sort_keys=True))
    timing = Path('target/cargo-timings/cargo-timing.html')
    if timing.exists():
        shutil.copyfile(timing, directory / f'{group}-timing.html')
    # Root-level library copies are outputs for the selected packages, not
    # inputs to consumers. Cargo uses the hashed libraries under deps instead.
    subprocess.run(['tar', '--exclude=ModuleCache', '--exclude=*.pcm', '--exclude=*.dia',
                    '--use-compress-program', 'zstd -1 -T0', '-cf',
                    str(directory / f'{group}.tar.zst'), 'target/release/deps',
                    'target/release/build', 'target/release/.fingerprint'], check=True)


def restore(directory):
    expected = identity()
    for group in ('ui', 'platform', 'backend'):
        actual = json.loads((directory / f'{group}.json').read_text())
        if actual != expected:
            raise SystemExit(f'{group}: run attempt, source, compiler, SDK or profile differs')
        subprocess.run(['tar', '--use-compress-program', 'unzstd', '-xf',
                        str(directory / f'{group}.tar.zst')], check=True)
    # Merging duplicate dependencies from separate workers otherwise makes a
    # dependency look newer than a crate that already compiled against it.
    set_times(Path('target/release'), source_time() + 1)


def download(directory):
    repository = os.environ['GITHUB_REPOSITORY']
    run = os.environ['GITHUB_RUN_ID']
    attempt = os.environ['GITHUB_RUN_ATTEMPT']
    pending = {f'cold-deps-{attempt}-{group}' for group in ('ui', 'platform', 'backend')}
    directory.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + 480
    while pending:
        pages = json.loads(output('gh', 'api', '--paginate', '--slurp',
                                  f'repos/{repository}/actions/runs/{run}/artifacts?per_page=100'))
        ready = {a['name'] for page in pages for a in page['artifacts'] if not a['expired']}
        for name in sorted(pending & ready):
            subprocess.run(['gh', 'run', 'download', run, '--repo', repository,
                            '--name', name, '--dir', str(directory)], check=True)
            pending.remove(name)
            print(f'Downloaded {name}', flush=True)
        if not pending:
            return
        jobs = json.loads(output('gh', 'api',
                                 f'repos/{repository}/actions/runs/{run}/attempts/{attempt}/jobs?per_page=100'))
        failed = [j['name'] for j in jobs['jobs'] if j['name'].startswith('Dependencies ')
                  and j['status'] == 'completed' and j['conclusion'] != 'success']
        if failed:
            raise SystemExit(f'Dependency jobs failed: {failed}')
        if time.monotonic() >= deadline:
            raise SystemExit(f'Timed out waiting for artifacts: {sorted(pending)}')
        time.sleep(5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'build', 'pack', 'restore', 'download'])
    parser.add_argument('--group', choices=['ui', 'platform', 'backend'])
    parser.add_argument('--directory', type=Path)
    args = parser.parse_args()
    if args.action in ('build', 'pack') and not args.group:
        parser.error('--group is required')
    if args.action in ('pack', 'restore', 'download') and args.directory is None:
        parser.error('--directory is required')
    if args.action == 'prepare':
        prepare()
    elif args.action == 'build':
        build(args.group)
    elif args.action == 'pack':
        pack(args.group, args.directory)
    elif args.action == 'restore':
        restore(args.directory)
    else:
        download(args.directory)


if __name__ == '__main__':
    main()
