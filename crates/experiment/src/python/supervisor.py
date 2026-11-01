"""The Python profile's supervisor (crates/experiment/src/python.rs): the container's first process under its init.

It copies the run's inputs into the scratch directory and starts the code, each as the code's user, then becomes
a user of its own, which the code can't signal, trace or read the memory of. It relays the code's output to the
runner in frames the code can't forge, since the code never writes to the container's output itself; it says how
much CPU time the container has used; and once the code has ended, it says how the code ended and whether the
scratch directory was full. It only classifies: every limit is the kernel's or the runner's, never its own.

Frames, on standard output: a tag byte, a four-byte big-endian length, then the payload. `o` and `e` are the
code's standard output and error; `c` the container's CPU time in microseconds, in decimal; `f` a collected file,
its path relative to the collected directory, a NUL, then its bytes; and `x`, last, a JSON object: `status`, the
code's exit status (negative for a signal, as subprocess has it), and `scratch`, whether the scratch directory was
full at any check. Its own errors go to standard error.

Argument: a JSON object with `argv`, the code's arguments after the interpreter's own flags; `cwd`, its working
directory; and `collect`, a directory under the scratch directory whose regular files go back once the code has
ended, or null.
"""

import ctypes
import json
import os
import selectors
import stat
import struct
import subprocess
import sys
import time

CODE = 65534
SELF = 65533
SCRATCH = "/scratch"
INPUTS = "/in"
POLL = 0.1
# The scratch directory counts as full with less than this left, of bytes or of files. Extended attributes take
# from the files' share, a file's worth a KiB, so one of the largest, 64 KiB, can be refused with 64 left.
SLACK_BYTES = 64 << 10
SLACK_FILES = 128
MAX_COLLECT = 32 << 20
ENV = {"HOME": SCRATCH, "TMPDIR": SCRATCH, "PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8"}
PR_CAPBSET_DROP = 24


def frame(tag, payload):
    view = memoryview(tag + struct.pack(">I", len(payload)) + payload)
    while view:
        view = view[os.write(1, view) :]


def cpu():
    with open("/sys/fs/cgroup/cpu.stat") as f:
        for line in f:
            key, _, value = line.partition(" ")
            if key == "usage_usec":
                return value.strip().encode()
    return b"0"


def full():
    s = os.statvfs(SCRATCH)
    return s.f_bavail * s.f_frsize < SLACK_BYTES or s.f_favail < SLACK_FILES


def as_code(argv, **kw):
    return subprocess.Popen(argv, user=CODE, group=CODE, extra_groups=[], env=ENV, umask=0o022, **kw)


def collect(root):
    """The regular files under `root`, without following a symlink or opening anything else, MAX_COLLECT at most."""
    total, stack = 0, [b""]
    while stack:
        rel = stack.pop()
        try:
            entries = list(os.scandir(os.path.join(root, rel) if rel else root))
        except OSError:
            continue
        for e in entries:
            path = os.path.join(rel, e.name) if rel else e.name
            try:
                if e.is_dir(follow_symlinks=False):
                    stack.append(path)
                    continue
                fd = os.open(e.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            except OSError:
                continue
            try:
                info = os.fstat(fd)
                if not stat.S_ISREG(info.st_mode) or total + info.st_size > MAX_COLLECT:
                    continue
                data = b""
                while len(data) < info.st_size:
                    chunk = os.read(fd, info.st_size - len(data))
                    if not chunk:
                        break
                    data += chunk
                total += len(data)
                frame(b"f", path + b"\0" + data)
            finally:
                os.close(fd)


def main():
    spec = json.loads(sys.argv[1])
    # Nothing started from here on can gain a capability back, the code included.
    libc = ctypes.CDLL(None, use_errno=True)
    with open("/proc/sys/kernel/cap_last_cap") as f:
        last = int(f.read())
    for cap in range(last + 1):
        if libc.prctl(PR_CAPBSET_DROP, cap, 0, 0, 0) != 0:
            raise OSError(ctypes.get_errno(), "dropping a capability from the bounding set")
    null = subprocess.DEVNULL
    copied = as_code(["cp", "-R", INPUTS + "/.", SCRATCH + "/"], stdin=null, stdout=null, stderr=subprocess.PIPE)
    copy_err = copied.communicate()[1]
    if copied.returncode != 0:
        os.setgroups([])
        os.setgid(SELF)
        os.setuid(SELF)
        sys.stderr.buffer.write(b"copying the inputs: " + copy_err[-2000:])
        frame(b"x", json.dumps({"status": None, "scratch": full()}).encode())
        return
    argv = [sys.executable, "-I", "-S", "-B", "-X", "utf8", *spec["argv"]]
    child = as_code(argv, cwd=spec["cwd"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    os.setgroups([])
    os.setgid(SELF)
    os.setuid(SELF)
    sel = selectors.DefaultSelector()
    sel.register(child.stdout, selectors.EVENT_READ, b"o")
    sel.register(child.stderr, selectors.EVENT_READ, b"e")
    was_full, next_poll = False, 0.0
    while child.poll() is None:
        if time.monotonic() >= next_poll:
            was_full = was_full or full()
            frame(b"c", cpu())
            next_poll = time.monotonic() + POLL
        if not sel.get_map():
            try:
                child.wait(POLL)
            except subprocess.TimeoutExpired:
                pass
            continue
        for key, _ in sel.select(POLL):
            data = os.read(key.fd, 65536)
            if data:
                frame(key.data, data)
            else:
                sel.unregister(key.fileobj)
    # What it wrote before it ended; not what anything it left behind goes on writing.
    end = time.monotonic() + 0.2
    while sel.get_map() and time.monotonic() < end:
        ready = sel.select(0)
        if not ready:
            break
        for key, _ in ready:
            data = os.read(key.fd, 65536)
            if data:
                frame(key.data, data)
            else:
                sel.unregister(key.fileobj)
    was_full = was_full or full()
    if spec.get("collect"):
        collect(os.fsencode(os.path.join(SCRATCH, spec["collect"])))
    frame(b"c", cpu())
    frame(b"x", json.dumps({"status": child.returncode, "scratch": was_full}).encode())


main()
