"""Bounded actual CLI ownership checks; only child processes receive signals."""
import ctypes
import errno
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

spec = importlib.util.spec_from_file_location("prior", Path(__file__).parents[1] / "src/apps/t01_pty.py")
prior = importlib.util.module_from_spec(spec)
sys.dont_write_bytecode = True
spec.loader.exec_module(prior)

ENCODER = r'''#!/usr/bin/env python3
import os,sys,time,subprocess
from pathlib import Path
root=Path(os.environ['T06_ROOT'])
args=sys.argv[1:]
if '--version' in args:
 print('x264 0.164.3108'); sys.exit(0)
if '--zones' in args and args[args.index('--zones')+1]=='0,0,crf=30':
 sys.stdin.buffer.read(); sys.exit(0)
out=Path(args[args.index('-o')+1])
def identity(pid):
 fields=Path(f'/proc/{pid}/stat').read_text().rsplit(') ',1)[1].split()
 return f'{pid} {fields[19]} {fields[2]}\n'
grandchild=subprocess.Popen(['sleep','300'])
(root/'pids').write_text(identity(os.getpid())+identity(grandchild.pid))
with (root/'invocations').open('a') as f: f.write(str(out)+'\n')
out.write_bytes(b'partial')
(root/'active').touch()
deadline=time.monotonic()+15
while not (root/'release').exists():
 if time.monotonic()>deadline: raise RuntimeError('encoder release watchdog')
 time.sleep(.01)
sys.stdin.buffer.read()
grandchild.terminate(); grandchild.wait()
out.write_bytes(b'complete')
(root/'completed').touch()
'''


def run(binary: str, template: Path, base: Path, mode: str, route: str) -> None:
    """Exercise dispatch through a joined E12 encoder/index owner and real input."""
    with tempfile.TemporaryDirectory(prefix='t06-', dir=base) as directory:
        root = Path(directory)
        config = json.loads(template.read_text())
        # All mutable products are case-local; source is an immutable sentinel.
        config['temp'] = str(root / 'work')
        config['condor']['output']['path'] = str(root / 'output.mkv')
        if mode.startswith('open') or mode.startswith('override'):
            data = {'VapourSynth': {
                'path': str(root/'media.mkv'),
                'import_method': {'DGDecNV': {'dgindexnv_executable': str(base/'encoder')}},
                'cache_path': None, 'filters': [],
            }}
            (root/'media.mkv').write_bytes(b'source-index-sentinel')
            if mode.startswith('open'):
                config['condor']['input'] = data
            else:
                config['condor']['sequence_config']['parallel_encoder']['input'] = data
        for name in ('parallel_encoder', 'scene_concatenator'):
            config['condor']['sequence_config'][name]['scenes_directory'] = str(root / 'work/scenes')
        job = root / 'job.json'
        job.write_text(json.dumps(config))
        (root / 'output.mkv').write_bytes(b'prior-output-sentinel')
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 12, 100, 0, 0))
        original = termios.tcgetattr(slave)
        env = os.environ.copy()
        env.pop('CONDOR_TEST_MODE', None)
        env.update(T06_ROOT=directory, TERM='xterm-256color')
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        errors = []
        terminal = bytearray()
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            child = subprocess.Popen([binary, '--config-file', str(job), '--logs', str(root/'events.log')]
                + (['encode'] if route == 'standalone' else ['--skip-scd']),
                stdin=slave, stdout=stdout, stderr=slave if mode.startswith('key') else stderr,
                env=env, preexec_fn=controlling_terminal)
            try:
                deadline = time.monotonic() + 25
                requests = 0
                observed = 0
                while child.poll() is None:
                    assert time.monotonic() < deadline, (mode, 'CLI watchdog', terminal)
                    if select.select([master], [], [], .01)[0]:
                        terminal.extend(os.read(master, 65536))
                    queries = terminal.count(b'\x1b[6n')
                    while observed < queries:
                        os.write(master, b'\x1b[1;1R'); observed += 1
                    if not (root/'active').exists():
                        continue
                    if mode.startswith('key') and not terminal.count(b'\x1b[?1049h'):
                        continue
                    if requests == 0:
                        if mode.startswith('key'):
                            os.write(master, b'\x03')
                        else:
                            child.send_signal(signal.SIGTERM if mode.startswith('term') else signal.SIGINT)
                        requests = 1
                        sent = time.monotonic()
                    elif requests == 1 and time.monotonic() - sent > .3:
                        assert child.poll() is None, 'first request killed active encoder'
                        assert not (root/'completed').exists()
                        if mode.endswith('graceful'):
                            (root/'release').touch()
                        elif mode == 'key-force':
                            os.write(master, b'\x03\x03\x03')
                        else:
                            child.send_signal(signal.SIGINT if mode.startswith('key') else signal.SIGTERM)
                        requests = 2
                status = child.wait(timeout=5)
                restored = termios.tcgetattr(slave)
                os.close(slave); slave = -1
                while True:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO: raise
                        break
                    if not chunk: break
                    terminal.extend(chunk)
                stdout.seek(0); data = stdout.read()
                stderr.seek(0); diagnostic = bytes(terminal) if mode.startswith('key') else stderr.read()
                assert status == (143 if mode.startswith('term') else 130), (mode, status, data, diagnostic)
                assert requests == 2
                assert restored == original, 'raw terminal mode not restored'
                if mode.startswith('key'):
                    assert terminal.count(b'\x1b[?1049h') == terminal.count(b'\x1b[?1049l') == 1
                    assert terminal.count(b'\x1b[?25h') == 1
                    assert diagnostic.index(b'\x1b[?25h') < diagnostic.index(b'Error:')
                else:
                    assert b'\x1b' not in diagnostic
                assert b'\x1b' not in data and b'cancel' not in data.lower(), data
                assert b'cancel' in diagnostic.lower() or b'interrupt' in diagnostic.lower(), diagnostic
                assert (root/'output.mkv').read_bytes() == b'prior-output-sentinel'
                assert 'CLI job cleanup complete' in (root/'events.log').read_text(), 'log guard did not flush final joined marker'
                assert len((root/'invocations').read_text().splitlines()) == 1, 'next scene started'
                saved = json.loads(job.read_text())['condor']['scenes']
                scenes = root/'work/scenes'
                if mode.endswith('graceful'):
                    assert (root/'completed').exists(), 'active encoder did not finish'
                    assert any(p.read_bytes() == b'complete' for p in scenes.iterdir() if p.is_file())
                    completed = saved[0]['sequence_data']['parallel_encoder']
                    waiting = saved[1]['sequence_data']['parallel_encoder']
                    assert completed['provenance'] and completed['completed_on'] and completed['bytes'] == 8, saved
                    assert waiting['provenance'] is None and waiting['completed_on'] is None and waiting['bytes'] is None, saved
                else:
                    assert not (root/'completed').exists()
                    assert not scenes.exists() or not any(p.is_file() and p.suffix == '.264' for p in scenes.iterdir())
                    assert all(scene['sequence_data']['parallel_encoder']['provenance'] is None for scene in saved), saved
                if mode.startswith(('open', 'override')):
                    assert not (root/'media.dgi').exists(), 'forced index published'
                    assert (root/'media.mkv').read_bytes() == b'source-index-sentinel'
                leader = int((root/'pids').read_text().split()[0])
                assert not Path(f'/proc/{leader}').exists(), 'direct child not reaped at outcome'
                print(f'{mode}/{route}: status={status}, joined/restored, one scene, sentinels, child reaped')
            finally:
                if child.poll() is None: child.kill()
                child.wait(timeout=5)
                if (root/'pids').exists():
                    prior.cleanup_fixtures((root/'pids').read_text().splitlines(), errors)
                if slave >= 0: os.close(slave)
                os.close(master)
                if errors:
                    raise AssertionError(f'{sys.exception()}; cleanup: {errors}')
        assert not errors, errors


def listener(binary: str, base: Path) -> None:
    """Two sequential listener invocations, each driving two actual display stages."""
    env = os.environ.copy()
    env['T06_LISTENER_ROOT'] = str(base)
    with tempfile.TemporaryFile() as output:
        child = subprocess.Popen([binary, '--exact', 'apps::tests::t06_registration_child', '--nocapture'],
                                 stdin=subprocess.DEVNULL, stdout=output, stderr=output, env=env)
        try:
            deadline = time.monotonic() + 25
            for invocation in range(2):
                for marker, event in ((f'ready-{invocation}', signal.SIGTERM if invocation == 0 else signal.SIGINT),
                                      (f'graceful-{invocation}', signal.SIGINT if invocation == 0 else signal.SIGTERM)):
                    while not (base/marker).exists():
                        assert child.poll() is None and time.monotonic() < deadline, ('listener marker', marker)
                        time.sleep(.005)
                    child.send_signal(event)
            status = child.wait(timeout=10)
            output.seek(0)
            text = output.read()
            assert status == 0, text
            assert b'1 passed; 0 failed' in text, text
        finally:
            if child.poll() is None: child.kill()
            child.wait(timeout=5)


if __name__ == '__main__' and len(sys.argv) == 4:
    listener(sys.argv[1], Path(sys.argv[2]))
elif __name__ == '__main__':
    assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
    binary, base = sys.argv[1:]
    base = Path(base)
    source = (base/'source.vpy').read_bytes()
    encoder = base/'encoder'
    encoder.write_text(ENCODER); encoder.chmod(0o755)
    failures = []
    for route in ('standalone', 'root'):
        for mode in ('key-graceful', 'key-force', 'key-mixed', 'int-graceful', 'int-force', 'term-graceful', 'term-force', 'open-force', 'override-force'):
            try:
                run(binary, base/'template.json', base, mode, route)
            except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
                failures.append(f'{mode}/{route}: {error}')
    assert (base/'source.vpy').read_bytes() == source
    assert not failures, '\n'.join(failures)
