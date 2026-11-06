#!/usr/bin/env bash
# Install the upstream native release for Cargo's TRANSCRIBE_DIR integration.
set -euo pipefail
cd "$(dirname "$0")/../.."
prefix="${1:?Pass an empty installation directory}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]]
[[ ! -e "$prefix" ]]
python3 - <<'PY'
import tomllib
with open('Cargo.lock', 'rb') as file:
    packages = tomllib.load(file)['package']
for name in ('transcribe-cpp', 'transcribe-cpp-sys'):
    versions = [p['version'] for p in packages if p['name'] == name]
    if versions != ['0.2.3']:
        raise SystemExit(f'Update the native artifact pin for {name}: {versions}')
PY
mkdir -p "$prefix/lib"
archive="$prefix/native.tar.gz"
curl --fail --silent --show-error --location --retry 3 \
  https://github.com/handy-computer/transcribe.cpp/releases/download/v0.2.3/transcribe-native-0.2.3-macos-arm64-metal.tar.gz \
  -o "$archive"
echo "1cc5e89d442f55c165a3f90e49090cec75cec349071d834c0aa656161afa9543  $archive" | shasum -a 256 -c -
tar -xzf "$archive" --strip-components=1 -C "$prefix"
for name in transcribe ggml ggml-base ggml-cpu ggml-metal; do
  mv "$prefix/lib$name.dylib" "$prefix/lib/"
done
python3 - "$prefix" <<'PY'
import json
import pathlib
import sys
prefix = pathlib.Path(sys.argv[1])
contract = json.loads((prefix / 'contract.json').read_text())
assert contract['version'] == '0.2.3', contract
assert set(contract['backends']) == {'metal', 'cpu'}, contract
# The upstream archive ships its libraries flat. Adapt it to the install-prefix
# manifest consumed by transcribe-cpp-sys. The dylib records its own dependencies.
(prefix / 'lib/transcribe-link.json').write_text(json.dumps({
    'shared': True, 'lib_dir': 'lib', 'libraries': ['transcribe'],
    'system_libs': [], 'frameworks': [], 'link_flags': [],
}))
PY
