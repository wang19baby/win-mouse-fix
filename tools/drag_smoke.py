"""Real Win32/loopback drag acceptance. Never connects to a user's app instance.

Requires an interactive Windows desktop and a built debug exe. Uses only stdlib.
Creates its own text window, restores cursor/focus, and gracefully closes its app.
"""
import argparse
import base64
import ctypes as C
from ctypes import wintypes as W
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
u = C.WinDLL('user32', use_last_error=True)
k = C.WinDLL('kernel32', use_last_error=True)
LRESULT = C.c_ssize_t
WNDPROC = C.WINFUNCTYPE(LRESULT, W.HWND, W.UINT, W.WPARAM, W.LPARAM)


def api(dll, name, result, *args):
    fn = getattr(dll, name)
    fn.restype, fn.argtypes = result, args
    return fn


create = api(u, 'CreateWindowExW', W.HWND, W.DWORD, W.LPCWSTR, W.LPCWSTR,
             W.DWORD, C.c_int, C.c_int, C.c_int, C.c_int, W.HWND, W.HMENU, W.HINSTANCE, W.LPVOID)
send = api(u, 'SendMessageW', LRESULT, W.HWND, W.UINT, W.WPARAM, W.LPARAM)
post = api(u, 'PostMessageW', W.BOOL, W.HWND, W.UINT, W.WPARAM, W.LPARAM)
subclass = api(u, 'SetWindowLongPtrW', C.c_void_p, W.HWND, C.c_int, C.c_void_p)
call = api(u, 'CallWindowProcW', LRESULT, C.c_void_p, W.HWND, W.UINT, W.WPARAM, W.LPARAM)
peek = api(u, 'PeekMessageW', W.BOOL, C.POINTER(W.MSG), W.HWND, W.UINT, W.UINT, W.UINT)
api(u, 'TranslateMessage', W.BOOL, C.POINTER(W.MSG))
api(u, 'DispatchMessageW', LRESULT, C.POINTER(W.MSG))
api(u, 'DestroyWindow', W.BOOL, W.HWND)
api(u, 'SetForegroundWindow', W.BOOL, W.HWND)
api(u, 'GetForegroundWindow', W.HWND)
api(u, 'SetFocus', W.HWND, W.HWND)
api(u, 'GetCursorPos', W.BOOL, C.POINTER(W.POINT))
api(u, 'SetCursorPos', W.BOOL, C.c_int, C.c_int)
api(u, 'ClientToScreen', W.BOOL, W.HWND, C.POINTER(W.POINT))
api(u, 'GetAsyncKeyState', C.c_short, C.c_int)
api(u, 'GetWindowThreadProcessId', W.DWORD, W.HWND, C.POINTER(W.DWORD))
api(u, 'FindWindowExW', W.HWND, W.HWND, W.HWND, W.LPCWSTR, W.LPCWSTR)
api(u, 'WindowFromPoint', W.HWND, W.POINT)
api(u, 'SetProcessDpiAwarenessContext', W.BOOL, C.c_void_p)
api(u, 'SetWindowPos', W.BOOL, W.HWND, W.HWND, C.c_int, C.c_int, C.c_int, C.c_int, W.UINT)
api(k, 'GetModuleHandleW', W.HMODULE, W.LPCWSTR)


def pump():
    msg = W.MSG()
    while peek(C.byref(msg), None, 0, 0, 1):
        u.TranslateMessage(C.byref(msg))
        u.DispatchMessageW(C.byref(msg))


def wait_for(predicate, timeout=4):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        pump()
        value = predicate()
        if value:
            return value
        time.sleep(.005)
    raise AssertionError('Timed out waiting for native/protocol state')


def settle(seconds=.1):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        pump()
        time.sleep(.005)


class WebSocket:
    def __init__(self, port, token):
        self.sock = socket.create_connection(('127.0.0.1', port), timeout=.2)
        self.buf = b''
        self.messages = []
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f'GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n'
                           f'Upgrade: websocket\r\nConnection: Upgrade\r\n'
                           f'Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
        header = b''
        while b'\r\n\r\n' not in header:
            header += self.sock.recv(4096)
        head, self.buf = header.split(b'\r\n\r\n', 1)
        assert head.startswith(b'HTTP/1.1 101'), 'WebSocket upgrade failed'
        self.sock.setblocking(False)
        self.send({'t': 'auth', 'token': token})
        self.until(lambda m: m.get('t') == 'status')

    def frame(self, payload, opcode=1, partial=False):
        mask = os.urandom(4)
        n = len(payload)
        length = bytes([n | 128]) if n < 126 else b'\xfe' + struct.pack('!H', n)
        frame = bytes([128 | opcode]) + length + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(frame[:-1] if partial else frame)

    def send(self, value):
        self.frame(json.dumps(value).encode())

    def poll(self):
        try:
            data = self.sock.recv(65536)
            if data:
                self.buf += data
        except BlockingIOError:
            pass
        while len(self.buf) >= 2:
            opcode, size = self.buf[0] & 15, self.buf[1] & 127
            offset = 2
            if size == 126:
                if len(self.buf) < 4:
                    break
                size, offset = struct.unpack('!H', self.buf[2:4])[0], 4
            elif size == 127:
                if len(self.buf) < 10:
                    break
                size, offset = struct.unpack('!Q', self.buf[2:10])[0], 10
            if len(self.buf) < offset + size:
                break
            payload = self.buf[offset:offset + size]
            self.buf = self.buf[offset + size:]
            if opcode == 1:
                message = json.loads(payload)
                # Never retain window inventories, thumbnails or credentials.
                if message.get('t') in ('drag', 'status', 'input_error'):
                    self.messages.append(message)
            elif opcode == 9:
                self.frame(payload, opcode=10)

    def until(self, predicate, timeout=4):
        def check():
            self.poll()
            for i, message in enumerate(self.messages):
                if predicate(message):
                    return self.messages.pop(i)
        return wait_for(check, timeout)

    def drag(self, action, ident):
        self.send({'t': 'drag', 'action': action, 'id': ident})

    def state(self, ident, state):
        return self.until(lambda m: m.get('t') == 'drag' and m.get('id') == ident and m.get('state') == state)

    def close(self):
        self.sock.close()


class Desktop:
    TEXT = 'Remote drag selects this entire sentence.'

    def __init__(self):
        self.cursor = W.POINT()
        assert u.GetCursorPos(C.byref(self.cursor))
        self.foreground = u.GetForegroundWindow()
        self.events = []
        self.window = create(0, 'STATIC', 'Win Mouse Fix - owned drag smoke', 0x10CF0000,
                             150, 150, 800, 200, None, None, k.GetModuleHandleW(None), None)
        assert self.window, C.get_last_error()
        self.edit = create(0, 'EDIT', self.TEXT, 0x50000004, 20, 40, 740, 80,
                           self.window, None, k.GetModuleHandleW(None), None)
        assert self.edit, C.get_last_error()

        @WNDPROC
        def observe(hwnd, message, wp, lp):
            # EDIT replaces a quick second WM_LBUTTONDOWN with WM_LBUTTONDBLCLK.
            if message in (0x201, 0x202, 0x203):
                self.events.append('up' if message == 0x202 else 'down')
            return call(self.old_proc, hwnd, message, wp, lp)
        self.callback = observe
        self.old_proc = subclass(self.edit, -4, C.cast(observe, C.c_void_p))
        assert self.old_proc, C.get_last_error()
        u.SetForegroundWindow(self.window)
        u.SetFocus(self.edit)
        settle()

    def position(self, x=2):
        assert u.SetWindowPos(self.window, W.HWND(-1), 0, 0, 0, 0, 0x53)
        u.SetForegroundWindow(self.window)
        u.SetFocus(self.edit)
        settle(.2)  # Let focus-change handlers settle before placing the cursor.
        point = W.POINT(x, 8)
        assert u.ClientToScreen(self.edit, C.byref(point))
        assert u.SetCursorPos(point.x, point.y)
        pump()
        actual = W.POINT()
        assert u.GetCursorPos(C.byref(actual))
        assert (actual.x, actual.y) == (point.x, point.y), 'Concurrent desktop input moved the smoke cursor'
        assert u.WindowFromPoint(actual) == self.edit, 'Owned EDIT window is covered or on an inactive desktop'
        self.events.clear()

    def selected(self):
        start, end = W.DWORD(), W.DWORD()
        send(self.edit, 0xB0, C.addressof(start), C.addressof(end))
        return self.TEXT[start.value:end.value]

    def select_sentence(self, ws, ident):
        target = W.POINT(600, 8)
        assert u.ClientToScreen(self.edit, C.byref(target))
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if self.selected() == self.TEXT:
                return
            current = W.POINT()
            assert u.GetCursorPos(C.byref(current))
            # Cursor feedback avoids depending on the user's Windows speed/EPP.
            dx = max(-8, min(8, target.x - current.x))
            dy = max(-8, min(8, target.y - current.y))
            ws.drag('renew', ident)
            ws.send({'t': 'move', 'dx': dx, 'dy': dy, 'drag_id': ident})
            settle(.01)
        raise AssertionError(f'Native selection mismatch: {self.selected()!r}; events={self.events}')

    def close(self):
        subclass(self.edit, -4, self.old_proc)
        u.DestroyWindow(self.window)
        u.SetCursorPos(self.cursor.x, self.cursor.y)
        if self.foreground:
            u.SetForegroundWindow(self.foreground)


def tray_window(pid):
    hwnd = None
    while True:
        hwnd = u.FindWindowExW(W.HWND(-3), hwnd, 'WinMouseFixClass', None)
        if not hwnd:
            return None
        owner = W.DWORD()
        u.GetWindowThreadProcessId(hwnd, C.byref(owner))
        if owner.value == pid:
            return hwnd


def source_state():
    digest = hashlib.sha256()
    paths = [ROOT / name for name in ('Cargo.toml', 'Cargo.lock', 'build.rs', 'resources.rc',
                                      '.gitignore', 'config.toml.example', 'package.json',
                                      'package-lock.json', 'playwright.config.js')]
    for folder in ('src', 'assets', 'tests/e2e', 'tools'):
        paths += [p for p in (ROOT / folder).rglob('*') if p.is_file() and '__pycache__' not in p.parts
                  and p.suffix in ('.rs', '.js', '.html', '.json', '.py', '.ps1', '.ico')]
    for path in sorted(paths):
        digest.update(path.relative_to(ROOT).as_posix().encode() + b'\0' + path.read_bytes() + b'\0')
    return digest.hexdigest()


def main(exe):
    u.SetProcessDpiAwarenessContext(C.c_void_p(-4))
    assert not (u.GetAsyncKeyState(1) & 0x8000), 'Release physical left button before smoke'
    desktop = Desktop()
    clients = []
    proc = None
    temp = None
    try:
        with tempfile.TemporaryDirectory(prefix='wmf-native-drag-', delete=False) as temp:
            work = Path(temp)
            shutil.copy2(exe, work / 'win-mouse-fix.exe')
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0))
                port = probe.getsockname()[1]
            configuration = f'''[general]\nstart_hidden = true\n[scroll]\nenabled = false\nsmooth = false\nspeed = 1.0\ninvert = false\n[buttons]\nenabled = false\n[remote]\nenabled = true\nlocal_only = true\nport = {port}\n'''
            (work / 'config.toml').write_text(configuration)
            with (work / 'stderr.log').open('wb') as log:
                proc = subprocess.Popen([str(work / 'win-mouse-fix.exe')], cwd=work,
                                        stdout=subprocess.DEVNULL, stderr=log)
                def ready():
                    assert proc.poll() is None, 'Isolated app exited before ready'
                    try:
                        with urllib.request.urlopen(f'http://127.0.0.1:{port}/manifest.json', timeout=.2) as response:
                            return response.status == 200 and (work / 'token.txt').exists()
                    except OSError:
                        return False
                try:
                    wait_for(ready, 20)
                except AssertionError as error:
                    diagnostic = re.sub(r'[a-f0-9]{64}', '[redacted]', (work / 'stderr.log').read_text(errors='replace'))
                    raise AssertionError('Isolated startup failed:\n' + diagnostic) from error
                token = (work / 'token.txt').read_text().strip()
                assert re.fullmatch(r'v2:[a-f0-9]{64}', token), 'Invalid generated token format'
                def connect():
                    ws = WebSocket(port, token[3:])
                    clients.append(ws)
                    return ws
                ws, foreign = connect(), connect()
                tray = wait_for(lambda: tray_window(proc.pid))
                desktop.position()
                ws.drag('start', 1)
                ws.state(1, 'started')
                wait_for(lambda: desktop.events == ['down'])
                ws.drag('start', 1)
                foreign.drag('start', 1)
                assert foreign.state(1, 'rejected')['reason'] == 'busy'
                foreign.drag('end', 1)
                foreign.send({'t': 'move', 'dx': 0, 'dy': 90, 'drag_id': 1})
                ws.send({'t': 'move', 'dx': 0, 'dy': 90, 'drag_id': 2})
                settle()
                assert desktop.events == ['down'] and u.GetAsyncKeyState(1) & 0x8000
                desktop.select_sentence(ws, 1)
                ws.drag('end', 1)
                ws.state(1, 'ended')
                wait_for(lambda: desktop.events == ['down', 'up'])
                assert not u.GetAsyncKeyState(1) & 0x8000
                ws.drag('start', 1)
                assert ws.state(1, 'rejected')['reason'] == 'stale_id'
                print('PASS normal selection, duplicate/stale ids and foreign-session isolation')

                for ident, mode in ((2, 'lease'), (3, 'disconnect'), (4, 'partial-frame')):
                    desktop.position()
                    ws.drag('start', ident)
                    ws.state(ident, 'started')
                    wait_for(lambda: desktop.events == ['down'])
                    started = time.monotonic()
                    if mode == 'disconnect':
                        ws.close()
                    elif mode == 'partial-frame':
                        ws.frame(b'{"t":"drag","action":"renew","id":4}', partial=True)
                    wait_for(lambda: desktop.events == ['down', 'up'], 3.5)
                    elapsed = time.monotonic() - started
                    assert not u.GetAsyncKeyState(1) & 0x8000
                    print(f'PASS {mode} release in {elapsed:.3f}s')
                    if mode in ('disconnect', 'partial-frame'):
                        ws.close()
                        ws = connect()

                desktop.position()
                ws.drag('start', 5)
                ws.state(5, 'started')
                wait_for(lambda: desktop.events == ['down'])
                # Real WM_COMMAND toggles only our process's server; no QR on stop.
                assert post(tray, 0x111, 1008, 0)
                wait_for(lambda: desktop.events == ['down', 'up'])
                assert proc.poll() is None and not u.GetAsyncKeyState(1) & 0x8000
                # Restart through the existing hot-reload path, avoiding QR UI.
                cfg = work / 'config.toml'
                cfg.write_text(configuration)
                wait_for(ready, 12)
                ws = connect()
                desktop.position()
                ws.drag('start', 1)
                ws.state(1, 'started')
                wait_for(lambda: desktop.events == ['down'])
                print('PASS server stop releases button; restart accepts fresh session')
                assert post(tray, 0x10, 0, 0)
                wait_for(lambda: proc.poll() is not None, 8)
                wait_for(lambda: desktop.events == ['down', 'up'])
                assert proc.returncode == 0 and not u.GetAsyncKeyState(1) & 0x8000
                print('PASS graceful process exit releases button (exit 0)')
    finally:
        for ws in clients:
            ws.close()
        try:
            if proc is not None and proc.poll() is None:
                tray = tray_window(proc.pid)
                if tray:
                    post(tray, 0x10, 0, 0)
                    try:
                        wait_for(lambda: proc.poll() is not None, 10)
                    except AssertionError:
                        proc.terminate()
                        proc.wait(timeout=5)
                        raise
                else:
                    proc.terminate()
                    proc.wait(timeout=5)
        finally:
            desktop.close()
        if temp is not None:
            shutil.rmtree(temp)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, default=ROOT / 'target/debug/win-mouse-fix.exe')
    parser.add_argument('--source-state', action='store_true')
    args = parser.parse_args()
    if args.source_state:
        print(source_state())
    else:
        main(args.exe.resolve())
