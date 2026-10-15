"""Kill three concrete drag-state regressions in a disposable source copy."""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix='wmf-drag-mutants-') as temp:
        work = Path(temp)
        for name in ('src', 'assets'):
            shutil.copytree(ROOT / name, work / name)
        for name in ('Cargo.toml', 'Cargo.lock', 'build.rs', 'resources.rc'):
            shutil.copy2(ROOT / name, work / name)
        source = work / 'src/remote_input.rs'
        original = source.read_text(encoding='utf-8')
        mutants = [
            ('retired id presses again', 'if id <= request.id', 'if id < request.id',
             'duplicate_and_retired_ids_cannot_press_twice'),
            ('foreign move accepted', 'Arc::ptr_eq(&held.control, control)',
             'true', 'foreign_and_stale_commands_cannot_touch_the_owner'),
            ('failed release forgotten', 'if sink.button(false) {', 'if { sink.button(false); true } {',
             'failed_up_is_retained_until_success_and_cannot_be_renewed_away'),
        ]
        command = ['cargo', 'test', '--offline', '--manifest-path', str(work / 'Cargo.toml'),
                   '--target-dir', str(ROOT / 'target/drag-mutants'), '--bin', 'win-mouse-fix']
        baseline = subprocess.run(command + ['remote_input::tests::'], capture_output=True, text=True)
        if baseline.returncode != 0:
            raise RuntimeError('Mutation baseline failed:\n' + baseline.stdout + baseline.stderr)
        for label, old, new, test in mutants:
            if original.count(old) != 1:
                raise AssertionError(f'Mutation anchor is not unique: {label}')
            source.write_text(original.replace(old, new), encoding='utf-8')
            result = subprocess.run(command + [test], capture_output=True, text=True)
            # Compile errors, zero selected tests and tool failures are NOT kills.
            if result.returncode != 101 or f'{test} ... FAILED' not in result.stdout:
                raise AssertionError(f'Mutant not behaviorally killed: {label}\n{result.stdout}{result.stderr}')
            print(f'KILLED {label}: {test}', flush=True)
            source.write_text(original, encoding='utf-8')
        restored = subprocess.run(command + ['remote_input::tests::'], capture_output=True, text=True)
        if restored.returncode != 0:
            raise RuntimeError('Restored source failed:\n' + restored.stdout + restored.stderr)
        print('PASS 3/3 concrete mutants killed; isolated baseline and restored source pass')


if __name__ == '__main__':
    main()
