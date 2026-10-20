#!/usr/bin/env python3
"""Integration regression on already-seen public files; not a new admission."""
import argparse
import json
import os
import platform
from pathlib import Path
import subprocess
import sys
import time
from run import digest, reference_segments, score
from worker_diagnostics import parse_worker_diagnostics

p = argparse.ArgumentParser(description=__doc__)
for name in ('probe', 'worker', 'fixtures', 'output'):
    p.add_argument('--' + name, type=Path, required=True)
a = p.parse_args()
root = Path(__file__).resolve().parent
manifest = json.loads((root / 'full-native-ami.json').read_text())
frozen = json.loads((root / 'reports/2026-10-18/full-native.json').read_text())
expected = {r['id']: r for r in frozen['rows'] if r['engine'] == 'candidate'}
runtime = dict(platform=platform.platform(), machine=platform.machine(),
               logical_cpu_count=os.cpu_count(),
               worker_build_rustflags=os.environ.get('MINUTES_SPEAKER_BUILD_RUSTFLAGS', 'unrecorded'))
if sys.platform.startswith('linux'):
    # The Linux helper deliberately links the system CPU BLAS implementation.
    # Bind that implementation too; an executable digest alone cannot identify
    # the shared library used for the measured numerical work.
    libraries = []
    listing = subprocess.check_output(['ldd', str(a.worker.resolve())], text=True)
    for line in listing.splitlines():
        if 'openblas' in line.lower() and '=>' in line:
            library = Path(line.split('=>', 1)[1].split(' (', 1)[0].strip()).resolve(strict=True)
            libraries.append(dict(name=library.name, sha256=digest(library)))
    assert libraries, 'Linux helper is missing the configured OpenBLAS runtime'
    runtime['openblas_libraries'] = libraries
identity = dict(probe_sha256=digest(a.probe), worker_sha256=digest(a.worker),
                runtime=runtime,
                manifest_sha256=digest(root / 'full-native-ami.json'),
                check_sha256=digest(Path(__file__)),
                source_sha256={str(p): digest(root.parents[2] / p) for p in [
                    Path('crates/core/src/diarize_full_worker.rs'),
                    Path('tests/eval/diarization/worker_diagnostics.py'),
                    Path('crates/core/src/bounded_child.rs'),
                    Path('crates/speaker-worker/src/main.rs'),
                    Path('crates/speaker-worker/src/stage_diagnostics.rs'),
                    Path('crates/speaker-worker/src/windows_handles.rs'),
                    Path('crates/core/examples/full_diarize_file.rs'),
                    Path('crates/core/src/diarize_full_protocol.rs'),
                    Path('crates/speaker-worker/Cargo.toml'),
                    Path('crates/speaker-worker/build.rs'),
                    Path('crates/speaker-worker/vcpkg.json'),
                    Path('.github/workflows/full-speaker-worker.yml'),
                    Path('crates/speaker-worker/Cargo.lock')]})
print(json.dumps(dict(identity=identity)), flush=True)
a.output.mkdir(parents=True, exist_ok=False)
rows = []
# Kill only our own synthetic probe after its supervised worker appears. The
# worker must retire on parent loss; no installed app or capture is involved.
parent_loss = None
if os.name == 'posix':
    first = manifest['recordings'][0]
    command = [str(a.probe.resolve()), str(a.worker.resolve()),
               str((a.fixtures / 'candidate-models').resolve()),
               str((a.fixtures / 'audio' / first['files'][0]['name']).resolve())]
    probe = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    worker_pid = None
    worker_command = None
    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            listing = subprocess.check_output(['ps', '-axo', 'pid=,ppid='], text=True)
            children = [int(row[0]) for line in listing.splitlines()
                        if len(row := line.split()) == 2 and int(row[1]) == probe.pid]
            if children:
                worker_pid = children[0]
                worker_command = subprocess.check_output(["ps", "-o", "command=", "-p", str(worker_pid)], text=True).strip()
                break
            if probe.poll() is not None:
                raise AssertionError('host exited before worker launch')
            time.sleep(.05)
        assert worker_pid is not None, 'worker did not launch before deadline'
        probe.kill()
        probe.wait(timeout=5)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            result = subprocess.run(['ps', '-o', 'stat=', '-p', str(worker_pid)], capture_output=True, text=True)
            if result.returncode or not result.stdout.strip() or result.stdout.strip().startswith('Z'):
                parent_loss = dict(passed=True, scope='actual worker retired after its own synthetic host exited; internal inference phase uninstrumented')
                break
            time.sleep(.05)
        assert parent_loss, 'worker survived supervising parent loss'
    finally:
        if probe.poll() is None:
            probe.kill()
            probe.wait(timeout=5)
        if worker_pid is not None and not parent_loss:
            identity = subprocess.run(['ps', '-o', 'command=', '-p', str(worker_pid)], capture_output=True, text=True)
            if worker_command and identity.stdout.strip() == worker_command:
                try:
                    os.kill(worker_pid, 9)
                except ProcessLookupError:
                    pass

for item in manifest['recordings']:
    audio = a.fixtures / 'audio' / item['files'][0]['name']
    assert digest(audio) == item['files'][0]['sha256']
    reference_path = a.fixtures / 'audio' / item['files'][1]['name']
    assert digest(reference_path) == item['files'][1]['sha256']
    print('Running ' + item['id'], flush=True)
    started = time.monotonic()
    with (a.output / (item['id'] + '.json')).open('xb') as out, (a.output / (item['id'] + '.stderr')).open('xb') as err:
        completed = subprocess.run([str(a.probe.resolve()), str(a.worker.resolve()),
                        str((a.fixtures / 'candidate-models').resolve()), str(audio.resolve())],
                       stdin=subprocess.DEVNULL, stdout=out, stderr=err, timeout=930)
    elapsed = time.monotonic() - started
    diagnostics = parse_worker_diagnostics(
        (a.output / (item['id'] + '.stderr')).read_text(errors='replace'))
    if completed.returncode:
        # Return only closed diagnostic labels and a bounded numeric status;
        # arbitrary worker stderr stays in the private local file.
        failure = dict(**identity, id=item['id'], processing_seconds=elapsed,
                       real_time_factor=elapsed / item['duration'],
                       **diagnostics,
                       exit_code=completed.returncode, gates=dict(engine_completed=False))
        (a.output / (item['id'] + '-regression.json')).write_text(json.dumps(failure, indent=2) + '\n')
        print(json.dumps(failure), flush=True)
        raise AssertionError('host/worker did not complete successfully; see bounded local stderr')
    result = json.loads((a.output / (item['id'] + '.json')).read_text())
    assert result['profile_namespace'] == 'speakrs_af8013f_cpu_67f963b6_anonymous_v1'
    scored = score(reference_segments(reference_path, item['duration']), result, item['duration'])
    gates = dict(der=scored['der'] <= expected[item['id']]['der'] + 0.01,
                 speaker_count=scored['predicted_speakers'] == 4,
                 runtime=elapsed / item['duration'] <= 0.25,
                 input_preserved=digest(audio) == item['files'][0]['sha256'])
    row = dict(id=item['id'], processing_seconds=elapsed,
               real_time_factor=elapsed / item['duration'], gates=gates, **diagnostics, **scored)
    # Preserve failed measurements too; a failing gate is evidence, not a reason
    # to lose the timing and exact executable/source identity.
    (a.output / (item['id'] + '-regression.json')).write_text(
        json.dumps(dict(**identity, **row), indent=2) + '\n')
    print(json.dumps(row), flush=True)
    assert all(gates.values()), 'frozen regression gates failed: ' + ', '.join(k for k, v in gates.items() if not v)
    rows.append(row)
report = dict(qualification='Already-seen public integration regression; not fresh model admission, live capture, ASR or named-profile qualification.',
              **identity, parent_loss=parent_loss, rows=rows)
(a.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
