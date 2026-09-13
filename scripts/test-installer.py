#!/usr/bin/env python3
"""Exercise installation sequencing without downloading or installing software."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
STUB = '''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ['INSTALL_TEST_LOG'], 'a') as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + '\\n')
sys.exit(1 if os.environ.get('INSTALL_TEST_FAIL') == name else 0)
'''

class InstallerTests(unittest.TestCase):
    def run_installer(self, **extra):
        with tempfile.TemporaryDirectory(prefix='briefcase-install-') as directory:
            root = Path(directory)
            bindir = root / 'bin'
            bindir.mkdir()
            for program in ('cargo', 'rustup', 'briefcase'):
                file = bindir / program
                file.write_text(STUB)
                file.chmod(0o755)
            log = root / 'calls.jsonl'
            env = {**os.environ, 'HOME': str(root), 'PATH': str(bindir) + os.pathsep + os.environ['PATH'],
                   'INSTALL_TEST_LOG': str(log), 'CARGO_HOME': str(root / 'cargo')}
            env.pop('BRIEFCASE_INSTALL_SOURCE', None)
            env.pop('BRIEFCASE_INSTALL_VERSION', None)
            env.update(extra)
            result = subprocess.run(['sh', str(ROOT / 'docs/install.sh')], env=env, text=True, capture_output=True)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            return result, calls

    def test_registry_install_starts_the_daemon_after_cargo(self):
        result, calls = self.run_installer(BRIEFCASE_INSTALL_VERSION='1.1.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0][:3], ['rustup', 'toolchain', 'install'])
        self.assertEqual(calls[1], ['cargo', '+1.98.0', 'install', 'briefcase-cli', '--root', calls[1][calls[1].index('--root') + 1], '--locked', '--version', '=1.1.0', '--bin', 'briefcase', '--force'])
        self.assertEqual(calls[2], ['briefcase', 'daemon', 'install'])

    def test_source_paths_are_single_arguments(self):
        source = '/tmp/briefcase source with spaces'
        result, calls = self.run_installer(BRIEFCASE_INSTALL_SOURCE=source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[1][calls[1].index('--path') + 1], source)

    def test_failed_install_never_starts_the_daemon_or_claims_success(self):
        result, calls = self.run_installer(INSTALL_TEST_FAIL='cargo')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call[0] == 'briefcase' for call in calls))
        self.assertNotIn('are installed', result.stdout)

if __name__ == '__main__':
    unittest.main()
