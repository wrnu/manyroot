"""The grader's host: runs one group's hidden tests with unittest, in the Python profile's child, whose scratch
directory holds the hidden tests and nothing of the code under test. The code under test is never imported or
run here:

- importing the package gives a proxy module. Each attribute, call, iteration and so on goes to the grader,
  which relays it to the server, a sandboxed child with the tree. Plain values come back as data and are
  compared here; other objects come back as proxies, which equal only themselves, and whose `in` is answered
  here from their items. Only the package and its submodules ever become proxy modules, and only when this
  process imports them (finding 734e5389e188).
- an exception the code raises comes back as its class, described with its bases. It is raised here as one
  class per class of the code's, of the same name and module, whose bases are the mirrors of its bases, so
  issubclass holds along the code's own hierarchy (finding 621c185d3e6f). A class of Python's own, builtins or
  the standard library, is raised as itself; but not StopIteration, StopAsyncIteration or anything of unittest,
  which would stop an iterator or skip a test here (finding d077f100ac33).
- subprocess.Popen, so subprocess.run and the rest, runs only `python -m <package> ...`, and not here: the
  grader runs it in a fresh sandboxed child with the tree, the files of the working directory, and the input,
  and gives back its output, its exit code and the working directory's files. This process's own directory is
  an empty one of its own, never the hidden tests' or one holding them, so a command is never given a hidden
  file, and the files it gives back are written only inside its working directory (findings 7f3dc66a8589,
  f13d1ff05b5f).

Arguments: the hidden tests' directory, the group's test file relative to it, the package, and an empty
directory to work in. Standard output carries only the lines to the grader: the requests, the number of tests,
each outcome and each test's end as they happen (finding e7d128464936), and last the counts; the replies come
on standard input.
"""

import base64
import builtins
import datetime
import decimal
import importlib.abc
import importlib.machinery
import importlib.util
import io
import json
import os
import subprocess
import sys
import types
import unittest

tests, group, package, home = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
TESTS = os.path.realpath(tests)
os.chdir(home)
to_grader = os.fdopen(os.dup(1), "w", encoding="utf-8")
from_grader = os.fdopen(os.dup(0), "r", encoding="utf-8")
null = os.open("/dev/null", os.O_RDONLY)
os.dup2(null, 0)
os.close(null)
os.dup2(2, 1)
sys.path.insert(0, tests)


class GraderError(Exception):
    """The code under test couldn't be reached: its process ended, or it sent something unreadable."""


def tell(kind, what):
    to_grader.write(kind + " " + json.dumps(what) + "\n")
    to_grader.flush()


def ask(kind, request):
    tell(kind, request)
    line = from_grader.readline()
    if not line:
        raise GraderError("the grader has gone")
    reply = json.loads(line)
    if "gone" in reply:
        raise GraderError("the code under test ended: " + reply["gone"])
    return reply


def server(request):
    reply = ask("S", request)
    if "raise" in reply:
        raise raised(reply)
    if "ok" not in reply:
        raise GraderError("an unreadable reply")
    return dec(reply["ok"])


# Classes the code may never raise as themselves: they would stop an iterator, or skip or stop a test, here.
NEVER = (StopIteration, StopAsyncIteration, unittest.SkipTest)
# Modules whose classes are never taken for real ones, though of the standard library.
NOT_REAL = {"unittest", "__main__", "importlib", "sys"}
classes = {}


def real_class(module, qualname):
    """Python's own exception class `module.qualname`, if it is one the code may raise as itself. Strictly
    (finding d1f7d7880644): only from a module of the standard library this process has already loaded, never
    importing one for the code; the qualname walked through classes only, never another module; and only a class
    whose own module and qualname are the ones claimed, an exception, and of no module in NOT_REAL."""
    found = sys.modules.get(module)
    if found is None or isinstance(found, Module) or module.split(".")[0] not in sys.stdlib_module_names:
        return None
    for part in qualname.split("."):
        found = getattr(found, part, None)
        if not isinstance(found, type):
            return None
    try:
        own = (type.__getattribute__(found, "__module__"), type.__getattribute__(found, "__qualname__"))
    except AttributeError:
        return None
    if own != (module, qualname) or own[0].split(".")[0] in NOT_REAL:
        return None
    if issubclass(found, Exception) and not issubclass(found, NEVER):
        return found
    return None


def exception_class(described, depth=0):
    """The class standing here for the code's class `described`, [module, qualname, [bases described]]: one
    for each, by name, with the mirrors of its bases as its own."""
    if depth > 64:
        raise GraderError("a class nested too deeply")
    module, qualname, bases = str(described[0]), str(described[1]), described[2]
    real = real_class(module, qualname)
    if real is not None:
        return real
    key = (module, qualname)
    if key not in classes:
        mirrored = []
        for base in bases:
            if [str(base[0]), str(base[1])] in (["builtins", "object"], ["builtins", "BaseException"]):
                continue
            cls = exception_class(base, depth + 1)
            if cls not in mirrored:
                mirrored.append(cls)
        name = qualname.rsplit(".", 1)[-1]
        namespace = {"__module__": module, "__qualname__": qualname}
        try:
            classes[key] = type(name, tuple(mirrored) or (Exception,), namespace)
        except TypeError:
            classes[key] = type(name, (mirrored[0],), namespace)
    return classes[key]


def raised(reply):
    cls = exception_class(reply["raise"][3])
    message = str(reply["message"])
    try:
        args = dec(reply["args"])
        e = cls(*args) if type(args) is tuple else cls(message)
    except Exception:
        try:
            e = cls(message)
        except Exception:
            # A class whose constructor takes other arguments, such as json.JSONDecodeError's (finding
            # ed723694b473): made without it, with the message as its arguments.
            e = cls.__new__(cls)
            e.args = (message,)
    return e


class Proxy:
    """An object of the code under test, by its server's generation and its number there. It equals only
    itself. One got as an attribute of a module of the package remembers which, so a server started after its
    own has ended finds it again by name (finding 5f5c3b4b25a9)."""

    __slots__ = ("_handle", "_origin")

    def __init__(self, handle):
        object.__setattr__(self, "_handle", handle)
        object.__setattr__(self, "_origin", None)

    def _ref(self):
        ref = ["h", self._handle[0], self._handle[1]]
        return ref + [self._origin] if self._origin else ref

    def __getattr__(self, name):
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        return server({"op": "getattr", "obj": self._ref(), "name": name})

    def __setattr__(self, name, value):
        raise AttributeError("objects of the code under test can't be changed from a test")

    def __call__(self, *args, **kwargs):
        return server({"op": "call", "obj": self._ref(), "args": [enc(a) for a in args],
                       "kwargs": {k: enc(v) for k, v in kwargs.items()}})

    def __iter__(self):
        return iter(server({"op": "list", "obj": self._ref()}))

    def __len__(self):
        return server({"op": "len", "obj": self._ref()})

    def __bool__(self):
        return server({"op": "bool", "obj": self._ref()})

    def __str__(self):
        return server({"op": "str", "obj": self._ref()})

    def __repr__(self):
        return server({"op": "repr", "obj": self._ref()})

    def __contains__(self, item):
        # From its items, compared here: never its own __contains__.
        return any(x is item or x == item for x in self)

    def __getitem__(self, key):
        return server({"op": "getitem", "obj": self._ref(), "key": enc(key)})

    def __eq__(self, other):
        return isinstance(other, Proxy) and other._handle == self._handle

    def __ne__(self, other):
        return not self.__eq__(other)

    def __hash__(self):
        return hash(("proxy", self._handle))


class Module(types.ModuleType):
    """A module of the package under test, reached through the server."""

    def __getattr__(self, name):
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        found = server({"op": "getattr", "obj": ["m", self.__name__], "name": name})
        if type(found) is Proxy:
            object.__setattr__(found, "_origin", [self.__name__, name])
        return found


def module(name):
    """A module of the package the server named: the one this process imported, or else one of its own, never
    put in sys.modules, so only an import here makes a proxy module importable."""
    if name != package and not name.startswith(package + "."):
        raise GraderError("the code under test named a module that isn't the package's")
    known = sys.modules.get(name)
    return known if isinstance(known, Module) else Module(name)


class Finder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def find_spec(self, name, path=None, target=None):
        if name == package or name.startswith(package + "."):
            return importlib.machinery.ModuleSpec(name, self, is_package=True)
        return None

    def create_module(self, spec):
        server({"op": "import", "name": spec.name})
        return Module(spec.name)

    def exec_module(self, m):
        pass


# A package whose name something else here already answers to, the standard library or the tests' own helpers,
# would make that a proxy, or the package that: refused.
if importlib.util.find_spec(package.split(".")[0]) is not None:
    print("the package's name is already importable here", file=sys.stderr)
    sys.exit(3)
sys.meta_path.insert(0, Finder())


def enc(v, depth=0):
    if depth > 64:
        raise ValueError("a value nested too deeply to send")
    t = type(v)
    if v is None:
        return ["n"]
    if t is bool:
        return ["b", v]
    if t is int:
        return ["i", str(v)]
    if t is float:
        return ["f", repr(v)]
    if t is str:
        return ["s", v]
    if t is bytes:
        return ["y", base64.b64encode(v).decode()]
    if t is decimal.Decimal:
        return ["D", str(v)]
    if t is datetime.date:
        return ["date", v.isoformat()]
    if t is datetime.datetime and (v.tzinfo is None or type(v.tzinfo) is datetime.timezone):
        return ["datetime", v.isoformat()]
    if t is datetime.time and v.tzinfo is None:
        return ["time", v.isoformat()]
    if t is datetime.timedelta:
        return ["timedelta", v.days, v.seconds, v.microseconds]
    if t in (list, tuple, set, frozenset):
        tag = {list: "l", tuple: "t", set: "set", frozenset: "frozenset"}[t]
        return [tag, [enc(x, depth + 1) for x in v]]
    if t is dict:
        return ["d", [[enc(k, depth + 1), enc(x, depth + 1)] for k, x in v.items()]]
    if t is Proxy:
        return v._ref()
    if t is Module:
        return ["m", v.__name__]
    raise TypeError("only plain data and the package's own objects can be given to the code under test")


def dec(v, depth=0):
    if depth > 64:
        raise GraderError("a value nested too deeply")
    tag = v[0]
    if tag == "n":
        return None
    if tag == "b":
        return v[1] is True
    if tag == "s":
        return str(v[1])
    if tag == "i":
        return int(v[1])
    if tag == "f":
        return float(v[1])
    if tag == "y":
        return base64.b64decode(v[1])
    if tag == "D":
        return decimal.Decimal(v[1])
    if tag == "date":
        return datetime.date.fromisoformat(v[1])
    if tag == "datetime":
        return datetime.datetime.fromisoformat(v[1])
    if tag == "time":
        return datetime.time.fromisoformat(v[1])
    if tag == "timedelta":
        return datetime.timedelta(days=v[1], seconds=v[2], microseconds=v[3])
    if tag in ("l", "t", "set", "frozenset"):
        kind = {"l": list, "t": tuple, "set": set, "frozenset": frozenset}[tag]
        return kind(dec(x, depth + 1) for x in v[1])
    if tag == "d":
        return {dec(k, depth + 1): dec(x, depth + 1) for k, x in v[1]}
    if tag == "h":
        return Proxy((int(v[1]), int(v[2])))
    if tag == "e":
        return exception_class(v[3])
    if tag == "m":
        return module(str(v[1]))
    raise GraderError("an unknown value")


# The most bytes of a working directory's files a command may be given or give back.
MAX_FILES = 32 << 20


def holds_tests(path):
    """Whether the directory `path` is, holds or is inside the hidden tests' directory."""
    real = os.path.realpath(path)
    return real == TESTS or TESTS.startswith(real + os.sep) or real.startswith(TESTS + os.sep)


def files_of(root):
    out, total = [], 0
    for d, dirs, names in os.walk(root):
        dirs[:] = [x for x in dirs if not os.path.islink(os.path.join(d, x))]
        for n in names:
            p = os.path.join(d, n)
            if os.path.islink(p) or not os.path.isfile(p):
                continue
            with open(p, "rb") as f:
                data = f.read(MAX_FILES + 1)
            total += len(data)
            if total > MAX_FILES:
                raise GraderError("the working directory holds too much to give a command")
            out.append([os.path.relpath(p, root), data.hex()])
    return out


class Command:
    """Stands for subprocess.Popen. Runs only `python -m <package> ...`, through the grader."""

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None, stderr=None, cwd=None,
                 env=None, text=None, universal_newlines=None, encoding=None, errors=None, shell=False, **_):
        argv = [os.fsdecode(a) for a in args] if isinstance(args, (list, tuple)) else None
        python = bool(argv) and (argv[0] == sys.executable or os.path.basename(argv[0]).startswith("python"))
        if shell or not python or len(argv) < 3 or argv[1] != "-m" or not (
                argv[2] == package or argv[2].startswith(package + ".")):
            raise PermissionError("in a hidden test, only `python -m %s ...` runs as a command" % package)
        self.args = args
        self._module, self._argv = argv[2], argv[3:]
        self._cwd = os.fspath(cwd) if cwd is not None else os.getcwd()
        if holds_tests(self._cwd):
            raise PermissionError("a command can't run where the hidden tests are")
        self._text = bool(text or universal_newlines or encoding or errors)
        self._encoding, self._errors = encoding or "utf-8", errors or "strict"
        self._out, self._err = stdout, stderr
        self._input = stdin.read() if hasattr(stdin, "read") else None
        self.stdin = self.stdout = self.stderr = None
        self.returncode = None
        self.pid = 0
        self._result = (b"", b"")

    def _run(self, given):
        data = given if given is not None else self._input
        if isinstance(data, str):
            data = data.encode(self._encoding, self._errors)
        reply = ask("C", {"module": self._module, "argv": self._argv, "files": files_of(self._cwd),
                          "stdin": (data or b"").hex()})
        back = {}
        for rel, b in reply["files"]:
            parts = str(rel).split("/")
            if any(p in ("", ".", "..") for p in parts):
                raise GraderError("the command gave back a file outside its directory")
            back["/".join(parts)] = bytes.fromhex(b)
        for rel, _ in files_of(self._cwd):
            if rel not in back:
                os.remove(os.path.join(self._cwd, rel))
        root = os.path.realpath(self._cwd)
        for rel, data in back.items():
            path = os.path.join(root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            # Only inside the working directory, and through no symlink.
            if not os.path.realpath(os.path.dirname(path)).startswith(root + os.sep) and \
                    os.path.realpath(os.path.dirname(path)) != root:
                raise GraderError("the command gave back a file outside its directory")
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(data)
        out, err = bytes.fromhex(reply["stdout"]), bytes.fromhex(reply["stderr"])
        if self._err == subprocess.STDOUT:
            out, err = out + err, b""
        self._result = (out, err)
        self.returncode = reply["code"]

    def _shown(self, data, piped):
        if piped != subprocess.PIPE:
            return None
        if self._text:
            return io.TextIOWrapper(io.BytesIO(data), self._encoding, self._errors, newline=None).read()
        return data

    def communicate(self, input=None, timeout=None):
        if self.returncode is None:
            self._run(input)
        out, err = self._result
        return self._shown(out, self._out), self._shown(err, self._err)

    def wait(self, timeout=None):
        if self.returncode is None:
            self._run(None)
        return self.returncode

    def poll(self):
        return self.returncode

    def kill(self):
        pass

    terminate = kill

    def send_signal(self, sig):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self.returncode is None:
            self.wait()


subprocess.Popen = Command

counts = {"pass": 0, "fail": 0, "error": 0, "skip": 0}


def outcome(kind):
    counts[kind] += 1
    tell("O", kind)


class Result(unittest.TestResult):
    def stopTest(self, test):
        super().stopTest(test)
        tell("D", 1)

    def addSuccess(self, test):
        outcome("pass")

    def addFailure(self, test, err):
        outcome("fail")

    def addError(self, test, err):
        outcome("error")

    def addSkip(self, test, reason):
        outcome("skip")

    def addExpectedFailure(self, test, err):
        outcome("pass")

    def addUnexpectedSuccess(self, test):
        outcome("fail")

    def addSubTest(self, test, subtest, err):
        if err is not None:
            outcome("fail" if issubclass(err[0], test.failureException) else "error")


name = "hidden_" + group[:-3].replace("/", "_")
spec = importlib.util.spec_from_file_location(name, os.path.join(tests, group))
try:
    loaded = importlib.util.module_from_spec(spec)
    sys.modules[name] = loaded
    spec.loader.exec_module(loaded)
    suite = unittest.defaultTestLoader.loadTestsFromModule(loaded)
    tell("N", suite.countTestCases())
    suite.run(Result())
except Exception:
    outcome("error")
tell("R", counts)
