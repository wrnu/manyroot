"""The grader's host: runs one group's hidden tests with unittest, in the Python profile's child, whose scratch
directory holds the hidden tests and nothing of the code under test. The code under test is never imported or
run here:

- importing the package gives a proxy module. Each attribute, call, iteration and so on goes to the grader,
  which relays it to the server, a sandboxed child with the tree. Values of exactly the plain types come back as
  data and are compared here; other objects, a plain type's subclass too, come back as proxies, which are never
  compared, hashed or ordered, not even with themselves (decision d1fc04a4f896), and whose `in` is answered here
  from their items. One exception, Python's own: a list, a tuple or `in` checks identity before `__eq__`, so one
  and the same proxy object, held twice by a test, matches itself there; every value got from the code is a new
  proxy, so two never do (finding c9626960bb86). The code's exception classes and the package's modules aren't
  proxies but the host's own, one for each name, and the server sends a name only for the object it gives there
  and the first sent under it (decision 0f7f0b4b048b): within one server a name stands for one object, unless the
  code rewrites its own server's state, which shares its process, the same limit as the server's every answer has
  (findings 899e1616f407, a1e72cb9c89d). Only the package and its submodules ever become proxy modules, and only
  when this process imports them (finding 734e5389e188).
- an io.StringIO given to the code, or standard output or error redirected to one around a call, is given
  as a stream of the server's holding the same text, and holds what the code wrote to it there once the call
  returns or raises (task 6a80d88373a5).
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
  f13d1ff05b5f). A working directory that holds the hidden tests, as a test's helper gets from `__file__`,
  is given as an empty one, and what the command writes there is dropped. An argument that is an absolute
  path, such as a temporary file's, is given as a path in the command's own directory: a file's bytes, or a
  directory's files, and what the command writes there goes back to it, never to one holding the hidden tests
  (task 6a80d88373a5).
- a module of the package has a `__file__`, and a package its `__path__`: their places in the tree, but in
  the host's own copy of the tree's layout under its directory, which holds the tree's directories and no file.
  So a helper that finds the project from the package's file, as `Path(ledger.__file__).parents[1]`, gets a
  directory of its own: never the tree's real one, nor one holding the hidden tests (task 6a80d88373a5).
- the hidden tests are importable as the package `tests`, as a project's own tests often are, and their
  helpers by their own names: a group's file is the module `tests.<its path>`, so `from tests.support import
  x` and `from .support import x` work as well as `from support import x`.

Arguments: the hidden tests' directory, the group's test file relative to it, the package, and an empty
directory to work in. Standard output carries only the lines to the grader: the requests, the number of tests,
each test's start, each outcome and each test's end as they happen (finding e7d128464936), a description of
each error outside any test (X: its class, message and frames, which the grader keeps for --diagnose alone,
task 6a80d88373a5), and last the counts; the replies come on standard input.
"""

import base64
import builtins
import datetime
import decimal
import fractions
import importlib.abc
import importlib.machinery
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import traceback
import types
import unicodedata
import unittest

tests, group, package, home = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
TESTS = os.path.realpath(tests)
os.chdir(home)
# The host's own temporary directory, holding no test, so what the code writes to a test's temporary file comes
# back (finding 8221427759ca); in the directory the host may give paths in.
os.makedirs(os.path.join(home, "tmp"), exist_ok=True)
tempfile.tempdir = os.path.realpath(os.path.join(home, "tmp"))
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


# The text streams given in the request being made, by number.
sent = []


def server(request):
    try:
        if request.get("op") == "call":
            for name in ("stdout", "stderr"):
                if type(getattr(sys, name)) is io.StringIO:
                    request[name] = enc(getattr(sys, name))
        if given.by:
            request.update(files=given.files(), dirs=given.roots(), paths=given.names)
        reply = ask("S", request)
        if given.by:
            given.back(given_back(reply.get("files", [])))
        for i, text, pos in reply.get("streams", []):
            if type(i) is int and 0 <= i < len(sent):
                s = sent[i]
                s.seek(0)
                s.truncate()
                s.write(str(text))
                s.seek(min(int(pos), len(str(text))))
    finally:
        sent.clear()
        given.clear()
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
    whose own module and qualname are the ones claimed, an exception, and of no module in NOT_REAL. A module in
    NOT_REAL is refused before the walk, and the walk reads each object's own `__dict__`, never `getattr`, so no
    module's lazy `__getattr__` imports anything for the code (finding 998cb1f3da18)."""
    top = module.split(".")[0]
    if top in NOT_REAL or top not in sys.stdlib_module_names:
        return None
    found = sys.modules.get(module)
    if found is None or isinstance(found, Module):
        return None
    for part in qualname.split("."):
        found = object.__getattribute__(found, "__dict__").get(part)
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
    """The class standing here for the code's class `described`, [module, qualname, [bases described], own]: one
    for each name, with the mirrors of its bases as its own. A class the server sent with `own`, its generation
    and handle, isn't the object its name gives there, or another was sent under that name first: it gets a class
    of its own, never Python's of that name nor another of the code's (decision 0f7f0b4b048b). The first such class
    the server sent under its module and qualname is kept by them, apart from the named ones, so a test's class of
    it still catches it after a restart (finding 90ae40b8ad52); any later one by its generation and handle. Of
    several under one module and qualname, the first sent after a restart takes that class, whichever it is
    (finding 30eb693032b7, eval-suite.md's restart residual)."""
    if depth > 64:
        raise GraderError("a class nested too deeply")
    module, qualname, bases = str(described[0]), str(described[1]), described[2]
    own = described[3] if len(described) > 3 else None
    if own is None:
        real = real_class(module, qualname)
        if real is not None:
            return real
        key = (module, qualname)
    elif len(own) > 2 and own[2] is True:
        key = ("unnamed", module, qualname)
    else:
        key = ("own", int(own[0]), int(own[1]))
    if key not in classes:
        mirrored = []
        for base in bases:
            root = [str(base[0]), str(base[1])] in (["builtins", "object"], ["builtins", "BaseException"])
            if root and len(base) > 3 and base[3] is None:
                continue
            cls = exception_class(base, depth + 1)
            if cls not in mirrored:
                mirrored.append(cls)
        name = qualname.rsplit(".", 1)[-1]
        namespace = {"__module__": module, "__qualname__": qualname, "__str__": mirrored_str,
                     "__init__": mirrored_init, "_grader_mirror": True}
        try:
            classes[key] = type(name, tuple(mirrored) or (Exception,), namespace)
        except TypeError:
            classes[key] = type(name, (mirrored[0],), namespace)
    return classes[key]


def mirrored_str(e):
    """A mirrored exception says what the code's said (task 6a80d88373a5)."""
    told = e.__dict__.get("_told")
    return told if type(told) is str else BaseException.__str__(e)


def mirrored_init(e, *args, **kwargs):
    """A test's own instance of a mirrored class: made by the code's own __init__ on the server, and given its
    arguments, attributes and str() from there (task 6a80d88373a5). A class known only as a base, never got
    from the package, is made here."""
    cls = type(e)
    ref = cls.__dict__.get("_grader_ref")
    if ref is None:
        BaseException.__init__(e, *args)
        return
    origin = cls.__dict__.get("_grader_origin")
    made = server({"op": "make", "obj": ref + [origin] if origin else ref, "args": [enc(a) for a in args],
                   "kwargs": {k: enc(v) for k, v in kwargs.items()}})
    told(e, made)


def told(e, reply):
    """Gives exception `e` what the code's said: its arguments, its message as its str(), its attributes."""
    message = str(reply["message"])
    try:
        args = dec(reply["args"])
        e.args = args if type(args) is tuple else (message,)
    except Exception:
        e.args = (message,)
    if type(e).__str__ is mirrored_str:
        e._told = message
    for name, value in reply.get("attrs", []):
        if type(name) is str and not name.startswith("_") and name != "args":
            try:
                setattr(e, name, dec(value))
            except Exception:
                pass


def raised(reply):
    cls = exception_class(reply["raise"][3])
    message = str(reply["message"])
    if cls.__dict__.get("_grader_mirror"):
        # Never made again by the code: told what the raised one was.
        e = cls.__new__(cls)
        told(e, reply)
        return e
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
    if type(e).__str__ is mirrored_str:
        e._told = message
    for name, value in reply.get("attrs", []):
        if type(name) is str and not name.startswith("_") and name != "args":
            try:
                setattr(e, name, dec(value))
            except Exception:
                pass
    return e


UNCOMPARED = ("the code's objects are never compared (decision d1fc04a4f896): compare plain values it "
              "gives, an attribute's, a method's or str()'s")
# The dunder names a proxy asks the code for, as the object's own.
NAMING = {"__name__", "__qualname__", "__annotations__"}


class Proxy:
    """An object of the code under test, by its server's generation and its number there. It is never compared,
    hashed or ordered (decision d1fc04a4f896). One got as an attribute of a module of the package remembers which,
    so a server started after its own has ended finds it again by name (finding 5f5c3b4b25a9)."""

    __slots__ = ("_handle", "_origin")

    def __init__(self, handle):
        object.__setattr__(self, "_handle", handle)
        object.__setattr__(self, "_origin", None)

    def _ref(self):
        ref = ["h", self._handle[0], self._handle[1]]
        return ref + [self._origin] if self._origin else ref

    def __getattr__(self, name):
        if name.startswith("__") and name.endswith("__") and name not in NAMING:
            raise AttributeError(name)
        return server({"op": "getattr", "obj": self._ref(), "name": name})

    # What the object says of itself, never this class's own (task 6a80d88373a5).
    @property
    def __module__(self):
        return server({"op": "getattr", "obj": self._ref(), "name": "__module__"})

    @property
    def __doc__(self):
        return server({"op": "getattr", "obj": self._ref(), "name": "__doc__"})

    def __dir__(self):
        return server({"op": "dir", "obj": self._ref()})

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

    # The code's objects are never compared, nor hashed (decision d1fc04a4f896): against each other, itself, or
    # a plain value, each raises in the test, never answering. A test compares plain values the code gives, an
    # attribute's, a method's or str()'s, which come here as data.
    def __eq__(self, other):
        raise TypeError(UNCOMPARED)

    def __ne__(self, other):
        raise TypeError(UNCOMPARED)

    def __hash__(self):
        raise TypeError(UNCOMPARED)


# The code's own operators never answer a test's check of a value (finding c823af485f19). An operator's result on
# the code's objects stays one of the code's, a proxy, even when it is a number there: so `x - 1.5` that the code
# says is 0.0 is never 0.0 here, and assertAlmostEqual's round of it raises. Ordering a proxy raises, against
# anything, one of the code's own objects too, rather than let the code's __lt__ say (decision d1fc04a4f896). And
# float(), int(), round() and math's functions on a proxy raise: a test compares plain values the code returns,
# such as an attribute's or a method's.
ORDERING = {"lt", "le", "gt", "ge"}


def binary(name, reflected=False):
    """The operator `name` on a proxy, done by the code: with `other` on the left when reflected."""

    def op(self, other):
        if name in ORDERING:
            raise TypeError(UNCOMPARED)
        try:
            mine, theirs = self._ref(), enc(other)
        except (TypeError, ValueError):
            return NotImplemented
        a, b = (theirs, mine) if reflected else (mine, theirs)
        out = server({"op": "binop", "name": name, "a": a, "b": b})
        if type(out) is not Proxy:
            raise GraderError("an operator's result that isn't the code's")
        return out

    return op


def unary(name):
    def op(self):
        out = server({"op": "unop", "name": name, "obj": self._ref()})
        if type(out) is not Proxy:
            raise GraderError("an operator's result that isn't the code's")
        return out

    return op


for name in ("add", "sub", "mul", "truediv", "floordiv", "mod", "pow"):
    setattr(Proxy, "__%s__" % name, binary(name))
    setattr(Proxy, "__r%s__" % name, binary(name, reflected=True))
for name in ORDERING:
    setattr(Proxy, "__%s__" % name, binary(name))
for name in ("neg", "pos", "abs"):
    setattr(Proxy, "__%s__" % name, unary(name))


class Module(types.ModuleType):
    """A module of the package under test, reached through the server."""

    def __dir__(self):
        return sorted(set(super().__dir__()) | set(server({"op": "dir", "obj": ["m", self.__name__]})))

    def __getattr__(self, name):
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        found = server({"op": "getattr", "obj": ["m", self.__name__], "name": name})
        if type(found) is Proxy:
            object.__setattr__(found, "_origin", [self.__name__, name])
        elif isinstance(found, type) and found.__dict__.get("_grader_mirror"):
            type.__setattr__(found, "_grader_origin", [self.__name__, name])
        return found

    def __setattr__(self, name, value):
        # Not a submodule bound here as it's imported: the package's own attribute of that name, such as a
        # function its __init__ imported from it, is the server's to say (task 6a80d88373a5).
        if type(value) is Module and value.__name__ == self.__name__ + "." + name:
            return
        super().__setattr__(name, value)


def module(name):
    """A module of the package the server named: the one this process imported, or else one of its own, never
    put in sys.modules, so only an import here makes a proxy module importable."""
    if name != package and not name.startswith(package + "."):
        raise GraderError("the code under test named a module that isn't the package's")
    known = sys.modules.get(name)
    return known if isinstance(known, Module) else Module(name)


# The host's own copy of the tree's layout: its directories, as the modules imported name them, and no file.
TREE = os.path.join(os.path.realpath(home), "tree")


def mirrored(place):
    """A module's place in the tree, `place` as the server said it, in the host's copy: None for none, or one
    that isn't a plain relative path to a `.py` file."""
    parts = place.split("/") if type(place) is str and place.endswith(".py") else []
    if not parts or any(p in ("", ".", "..") or "\\" in p or "\0" in p for p in parts):
        return None
    path = os.path.join(TREE, *parts)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    return path


class Finder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def find_spec(self, name, path=None, target=None):
        if name == package or name.startswith(package + "."):
            # Only a module the tree has: as `importlib.util.find_spec` would say of it there.
            if not server({"op": "exists", "name": name}):
                return None
            return importlib.machinery.ModuleSpec(name, self, is_package=True)
        return None

    def create_module(self, spec):
        place = server({"op": "import", "name": spec.name})
        where = mirrored(place)
        if where is not None:
            spec.origin, spec.has_location = where, True
            if os.path.basename(where) == "__init__.py":
                spec.submodule_search_locations = [os.path.dirname(where)]
        return Module(spec.name)

    def exec_module(self, m):
        pass


# A package whose name something else here already answers to, the standard library or the tests' own helpers,
# would make that a proxy, or the package that: refused.
if importlib.util.find_spec(package.split(".")[0]) is not None:
    print("the package's name is already importable here", file=sys.stderr)
    sys.exit(3)
class Planted(importlib.abc.MetaPathFinder):
    """Refuses to import any module from the host's own directory (finding 8908d9699820). Everything the code
    can write here is there, the copy of the tree's layout, a command's working directory and the temporary
    files included, so no module the code planted is ever run here, wherever a test points sys.path."""

    def find_spec(self, name, path=None, target=None):
        spec = importlib.machinery.PathFinder.find_spec(name, path, target)
        if spec is None:
            return None
        places = [spec.origin] if spec.origin else []
        places += list(spec.submodule_search_locations or [])
        if any(os.path.realpath(p).startswith(HOME + os.sep) for p in places if isinstance(p, str)):
            raise ImportError("a module the code under test could have written is never imported here", name=name)
        return None


HOME = os.path.realpath(home)
sys.meta_path.insert(0, Planted())
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
        # A path such as a temporary file's: given, as the server's own (task 6a80d88373a5).
        shown = given.map(v) if v.startswith("/") else None
        return ["p", shown] if shown else ["s", v]
    if t is bytes:
        return ["y", base64.b64encode(v).decode()]
    if t is decimal.Decimal:
        return ["D", str(v)]
    if t is fractions.Fraction:
        return ["Q", str(v.numerator), str(v.denominator)]
    if t is complex:
        return ["c", repr(v.real), repr(v.imag)]
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
    if isinstance(v, os.PathLike) and type(os.fspath(v)) is str:
        shown = given.map(os.fspath(v))
        return ["p", shown] if shown else ["s", os.fspath(v)]
    if t is Proxy:
        return v._ref()
    if t is Module:
        return ["m", v.__name__]
    if t is io.StringIO:
        for i, s in enumerate(sent):
            if s is v:
                return ["sio", i, v.getvalue(), v.tell()]
        sent.append(v)
        return ["sio", len(sent) - 1, v.getvalue(), v.tell()]
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
    if tag == "Q":
        return fractions.Fraction(int(v[1]), int(v[2]))
    if tag == "c":
        return complex(float(v[1]), float(v[2]))
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
        cls = exception_class(v[3])
        if cls.__dict__.get("_grader_mirror"):
            # The code's class, by its handle, so a test's instance of it is made there.
            type.__setattr__(cls, "_grader_ref", ["h", int(v[1]), int(v[2])])
        return cls
    if tag == "made":
        return v[1]
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


class Mounts:
    """The directories that absolute paths name, each given as PATHS<n>: to a command in its own directory, to a
    call in the server's. A directory's files, or of a file's directory only the files named, and what was
    written there goes back. Only a path inside the temporary directory or this host's own, never one that is,
    holds or is inside the hidden tests' (task 6a80d88373a5)."""

    def __init__(self):
        self.clear()

    def clear(self):
        # (directory, whole) -> [its name as given, the names of its files given]; each path, as given and as is.
        self.by, self.names = {}, []

    def map(self, arg):
        """`arg`'s name as given, or None for one that isn't such a path."""
        if not os.path.isabs(arg) or "\0" in arg or holds_tests(arg):
            return None
        real = os.path.realpath(arg)
        if not any(real.startswith(r + os.sep) for r in ROOTS):
            return None
        whole = os.path.isdir(arg)
        if not whole and not os.path.isdir(os.path.dirname(arg)):
            return None
        mount = os.path.realpath(arg if whole else os.path.dirname(arg))
        key = (mount, whole)
        if key not in self.by:
            self.by[key] = ["%s%d" % (PATHS, len(self.by)), set()]
        root, names = self.by[key]
        shown = root if whole else root + "/" + os.path.basename(arg)
        if not whole:
            names.add(os.path.basename(arg))
        self.names.append((shown, arg))
        return shown

    def roots(self):
        return sorted(root for root, _ in self.by.values())

    def files(self):
        files = []
        for (mount, whole), (root, names) in self.by.items():
            if whole:
                files += [[root + "/" + rel, data] for rel, data in files_of(mount)]
                continue
            for name in sorted(names):
                path = os.path.join(mount, name)
                if os.path.isfile(path) and not os.path.islink(path):
                    with open(path, "rb") as f:
                        files.append([root + "/" + name, f.read(MAX_FILES + 1).hex()])
        if sum(len(f[1]) for f in files) > 2 * MAX_FILES:
            raise GraderError("the paths given hold too much")
        return files

    def back(self, back):
        """Writes back what was under each mount in `back` (relative path -> bytes); the rest is returned."""
        roots = set(self.roots())
        for (mount, whole), (root, _) in self.by.items():
            theirs = {rel[len(root) + 1:]: b for rel, b in back.items() if rel.startswith(root + "/")}
            if holds_tests(mount):
                # A file beside the hidden tests, such as the project's own: given, and nothing kept.
                continue
            # A file's directory: only what was made or changed, never its other files removed.
            write_back(mount, theirs, files_of(mount) if whole else [])
        return {rel: b for rel, b in back.items() if rel.split("/")[0] not in roots}

    def shown_again(self, data):
        """`data`, bytes, with each path as given saying the test's own again, the longest first."""
        for shown, arg in sorted(self.names, key=lambda n: -len(n[0])):
            data = data.replace(shown.encode(), os.fsencode(arg))
        return data


def given_back(files):
    """Files given back, by plain relative path."""
    back = {}
    for rel, b in files:
        parts = str(rel).split("/")
        if any(p in ("", ".", "..") for p in parts):
            raise GraderError("a file given back outside its directory")
        back["/".join(parts)] = bytes.fromhex(b)
    return back


class Command:
    """Stands for subprocess.Popen. Runs only `python -m <package> ...`, through the grader."""

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None, stderr=None, cwd=None,
                 env=None, text=None, universal_newlines=None, encoding=None, errors=None, shell=False, **_):
        argv = [os.fsdecode(a) for a in args] if isinstance(args, (list, tuple)) else None
        python = bool(argv) and (argv[0] == sys.executable or os.path.basename(argv[0]).startswith("python"))
        # Harmless interpreter flags before -m, such as -B: the command runs as the profile runs any.
        at = 1
        while python and at < len(argv) and FLAGS.match(argv[at]):
            at += 1
        if shell or not python or len(argv) < at + 2 or argv[at] != "-m" or not (
                argv[at + 1] == package or argv[at + 1].startswith(package + ".")):
            raise PermissionError("in a hidden test, only `python -m %s ...` runs as a command" % package)
        self.args = args
        self._module = argv[at + 1]
        argv = argv[:1] + argv[at:]
        self._cwd = os.fspath(cwd) if cwd is not None else os.getcwd()
        # Where the hidden tests are, the command gets an empty directory instead, and keeps nothing it writes.
        self._given_cwd = not holds_tests(self._cwd)
        self._mounts = Mounts()
        self._argv = [self._mounts.map(a) or a for a in argv[3:]]
        self._text = bool(text or universal_newlines or encoding or errors)
        self._encoding, self._errors = encoding or "utf-8", errors or "strict"
        self._out, self._err = stdout, stderr
        self._input = stdin.read() if hasattr(stdin, "read") else None
        self.stdin = self.stdout = self.stderr = None
        self.returncode = None
        self.pid = 0
        self._result = (b"", b"")

    def _files(self):
        roots = set(self._mounts.roots())
        files = files_of(self._cwd) if self._given_cwd else []
        return [f for f in files if f[0].split("/")[0] not in roots] + self._mounts.files()

    def _run(self, given):
        data = given if given is not None else self._input
        if isinstance(data, str):
            data = data.encode(self._encoding, self._errors)
        reply = ask("C", {"module": self._module, "argv": self._argv, "files": self._files(),
                          "dirs": self._mounts.roots(), "stdin": (data or b"").hex()})
        back = given_back(reply["files"])
        roots = set(self._mounts.roots())
        if self._given_cwd:
            write_back(self._cwd, {rel: b for rel, b in back.items() if rel.split("/")[0] not in roots},
                       files_of(self._cwd))
        # Then the paths it was given, so what it wrote to one is kept over the working directory's copy of it.
        self._mounts.back(back)
        out = self._mounts.shown_again(bytes.fromhex(reply["stdout"]))
        err = self._mounts.shown_again(bytes.fromhex(reply["stderr"]))
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


def write_back(directory, back, before):
    """Makes `directory`'s files those of `back`, by path relative to it: those in `before` and not in `back`
    removed. Only inside it, through no symlink, and never into the hidden tests."""
    root = os.path.realpath(directory)
    if holds_tests(root):
        raise GraderError("the command gave back a file where the hidden tests are")
    for rel, _ in before:
        if rel not in back:
            os.remove(os.path.join(root, rel))
    for rel, data in back.items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if not os.path.realpath(os.path.dirname(path)).startswith(root + os.sep) and \
                os.path.realpath(os.path.dirname(path)) != root:
            raise GraderError("the command gave back a file outside its directory")
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(data)


# Interpreter flags a command may give before -m: its environment is never given, so none changes what runs.
FLAGS = re.compile(r"-[BIEsSuOqdbv]+$")
# The name in a command's directory, or the server's for a call, under which it's given the paths it's given.
PATHS = ".grader-path-"
# Where a path must be to be given: inside the temporary directory, or this host's own.
ROOTS = [os.path.realpath(tempfile.gettempdir()), os.path.realpath(home)]
# The paths given in the request being made.
given = Mounts()

subprocess.Popen = Command

counts = {"pass": 0, "fail": 0, "error": 0, "skip": 0}


def outcome(kind):
    counts[kind] += 1
    tell("O", kind)


def where(path):
    """A frame's file as a diagnosis names it: a hidden test's relative to the tests' directory, this host's
    by its own name, anything else in full."""
    real = os.path.realpath(path)
    if real.startswith(TESTS + os.sep):
        return "tests/" + os.path.relpath(real, TESTS)
    if real == os.path.realpath(__file__):
        return "host.py"
    return path


UNITTEST = os.path.realpath(os.path.dirname(unittest.__file__)) + os.sep


# The categories of characters a diagnosis shows as an escape (finding 052914fb0445): controls, formats (the tag
# characters, direction marks and overrides), private use, surrogates, unassigned, line and paragraph separators.
HIDDEN = {"Cc", "Cf", "Co", "Cs", "Cn", "Zl", "Zp"}


# And these, invisible though of other categories (finding 7df26ede5a8b): the variation selectors, the
# combining grapheme joiner, the Hangul fillers and the Braille blank; and any whose name says BLANK or FILLER.
INVISIBLE = [(0xFE00, 0xFE0F), (0x180B, 0x180F), (0xE0100, 0xE01EF), (0x034F, 0x034F), (0x115F, 0x1160),
             (0x3164, 0x3164), (0xFFA0, 0xFFA0), (0x2800, 0x2800), (0x17B4, 0x17B5)]


def hidden(c):
    if unicodedata.category(c) in HIDDEN:
        return True
    o = ord(c)
    if any(lo <= o <= hi for lo, hi in INVISIBLE):
        return True
    name = unicodedata.name(c, "")
    return "BLANK" in name or "FILLER" in name


def shown(text):
    """`text` with each character of HIDDEN, or invisible, shown as \\u{...}, as the grader shows one."""
    return "".join("\\u{%x}" % ord(c) if hidden(c) else c for c in text)


def diagnose(err, test=None):
    """Tells the grader what an error outside any test was, or why test `test` (its index) erred or failed: the
    exception's class, its message (a test's cut to 200 characters) and its frames, file:line."""
    cls, value, tb = err
    # Not unittest's own frames, which every test's start with.
    frames = [[where(f.filename), f.lineno, f.name] for f in traceback.extract_tb(tb)
              if not os.path.realpath(f.filename).startswith(UNITTEST)][-40:]
    try:
        message = str(value)[:300]
    except Exception:
        message = "?"
    name = "%s.%s" % (getattr(cls, "__module__", "?"), getattr(cls, "__qualname__", "?"))
    described = {"class": shown(name[:100]), "message": shown(message),
                 "frames": [[shown(str(f)[:200]), line, shown(str(fn)[:200])] for f, line, fn in frames]}
    if test is not None:
        described["test"] = test
    tell("X", described)


# Each test's index in the suite's order: an outcome is told after its test's B, so one outside any test (a
# class or module fixture's error) is never charged to a test that ran (finding bcecc7c3f6e3).
index = {}


def flat(suite):
    for t in suite:
        if isinstance(t, unittest.TestSuite):
            yield from flat(t)
        else:
            yield t


class Result(unittest.TestResult):
    def startTest(self, test):
        super().startTest(test)
        tell("B", index.get(id(test), -1))

    def stopTest(self, test):
        super().stopTest(test)
        tell("D", 1)

    def addSuccess(self, test):
        outcome("pass")

    def addFailure(self, test, err):
        diagnose(err, index.get(id(test), -1))
        outcome("fail")

    def addError(self, test, err):
        # A class's or module's fixture: an error outside any test.
        if isinstance(test, unittest.suite._ErrorHolder):
            diagnose(err)
        else:
            diagnose(err, index.get(id(test), -1))
        outcome("error")

    def addSkip(self, test, reason):
        outcome("skip")

    def addExpectedFailure(self, test, err):
        outcome("pass")

    def addUnexpectedSuccess(self, test):
        outcome("fail")

    def addSubTest(self, test, subtest, err):
        if err is not None:
            diagnose(err, index.get(id(test), -1))
            outcome("fail" if issubclass(err[0], test.failureException) else "error")


def tests_package():
    """The hidden tests' directory as the package `tests`: its `__init__.py` if it has one, else a namespace."""
    init = os.path.join(tests, "__init__.py")
    if os.path.isfile(init):
        spec = importlib.util.spec_from_file_location("tests", init, submodule_search_locations=[tests])
    else:
        spec = importlib.machinery.ModuleSpec("tests", None, is_package=True)
        spec.submodule_search_locations = [tests]
    made = importlib.util.module_from_spec(spec)
    sys.modules["tests"] = made
    if spec.loader is not None:
        spec.loader.exec_module(made)


name = "tests." + group[:-3].replace("/", ".")
spec = importlib.util.spec_from_file_location(name, os.path.join(tests, group))
try:
    tests_package()
    if name.count(".") > 1:
        importlib.import_module(name.rsplit(".", 1)[0])
    loaded = importlib.util.module_from_spec(spec)
    sys.modules[name] = loaded
    spec.loader.exec_module(loaded)
    suite = unittest.defaultTestLoader.loadTestsFromModule(loaded)
    tests_in_order = list(flat(suite))
    index.update((id(t), i) for i, t in enumerate(tests_in_order))
    tell("N", len(tests_in_order))
    suite.run(Result())
except Exception:
    diagnose(sys.exc_info())
    outcome("error")
tell("R", counts)
