"""The grader's server: the code under test's side. It runs in the Python profile's child with the tree on its
path, and answers the host's requests about the package one JSON line at a time: standard input in, a copy of
standard output out. The code's own output goes to standard error, and its standard input is /dev/null.

Arguments: the tree, the package, and this server's generation. Every value goes back as data when its type is
exactly one of the plain types below, checked by identity; and otherwise, a plain type's subclass too, as a
handle, a number standing for the object here, tagged with the generation, which the host never compares
(decision d1fc04a4f896). So the host compares only data it decoded
itself, the code's own __eq__ or __contains__ never answers for it, and a handle from a server that has ended is
refused by the next one rather than taken for another object (finding fe2549db89ee).
"""

import base64
import datetime
import decimal
import fractions
import importlib
import importlib.util
import io
import operator
import json
import os
import shutil
import sys
import types

tree, package, generation = sys.argv[1], sys.argv[2], int(sys.argv[3])
requests = os.fdopen(os.dup(0), "r", encoding="utf-8")
replies = os.fdopen(os.dup(1), "w", encoding="utf-8")
null = os.open("/dev/null", os.O_RDONLY)
os.dup2(null, 0)
os.close(null)
os.dup2(2, 1)
sys.path.insert(0, tree)

DEPTH = 64
handles = []
# The host's text streams given in this request, by its number for each: what they hold after it goes back.
streams = {}
# Where the paths given in this request are, if any (task 6a80d88373a5): fresh for each, and gone after it.
SCRATCH = os.path.dirname(os.path.realpath(tree))
area = None
# The most bytes of files given back.
MAX_FILES = 32 << 20


def plain(rel):
    parts = str(rel).split("/")
    if any(p in ("", ".", "..") or "\0" in p for p in parts):
        raise ValueError("a path that isn't plain")
    return os.path.join(area, *parts)


def lay_out(r, n):
    """Writes the files and directories of request `r`, the `n`th, to a fresh directory: the area."""
    global area
    area = os.path.join(SCRATCH, "given", str(n))
    os.makedirs(area)
    for d in r.get("dirs", []):
        os.makedirs(plain(d), exist_ok=True)
    for rel, data in r.get("files", []):
        path = plain(rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(bytes.fromhex(data))


def gathered():
    """The area's files after a request, by path relative to it, at most MAX_FILES bytes in all."""
    out, total = [], 0
    for d, dirs, names in os.walk(area):
        dirs[:] = [x for x in dirs if not os.path.islink(os.path.join(d, x))]
        for name in names:
            p = os.path.join(d, name)
            if os.path.islink(p) or not os.path.isfile(p):
                continue
            with open(p, "rb") as f:
                data = f.read(MAX_FILES + 1)
            total += len(data)
            if total > MAX_FILES:
                return out
            out.append([os.path.relpath(p, area).replace(os.sep, "/"), data.hex()])
    return out


class Stale(Exception):
    """A handle of a server that has ended."""


def handle(obj):
    handles.append(obj)
    return len(handles) - 1


# The object first reported under each name for this server's life (decision 0f7f0b4b048b): ("m", name) for a
# module, ("e", module, qualname) for a class. A module or a class goes to the host by its name only if it is the
# object the name gives and the first reported under it, so two of the code's modules or classes, which the host
# keeps one of for each name, match there only when they are one object here.
named = {}


def first(key, obj):
    """Whether `obj` is the object first reported under `key`, as it now is if none was."""
    return named.setdefault(key, obj) is obj


# Each class that doesn't go by its name, by identity: its one handle for this server's life, so it is one class in
# the host however often it comes, and only another object is another class (finding deeada376fa8). The handles
# hold the classes, so no id here is ever another's.
owned = {}


def own_handle(cls):
    """The one handle of class `cls`, which doesn't go by its name."""
    h = owned.get(id(cls))
    if h is None:
        h = owned[id(cls)] = handle(cls)
    return h


# The names each class that doesn't go by its name carries across a restart by, taken at its first send and
# fixed for this server's life (findings 9ed85ad61c06, b8aa42f090fd): ["b", "module.attribute"] for each
# attribute of the package's modules bound to it then, in order, except one another object was sent under first;
# and ["u", module, qualname] if it is the first such class sent under them. The host keeps its class under all
# of them, so after a restart it is found again by any it still has.
carries = {}


def bound(cls):
    """Where class `cls` is bound among the package's modules, of exactly the module type: "module.attribute" for
    each, in order. By identity, reading each module's own `__dict__`."""
    found = []
    for name, m in list(sys.modules.items()):
        if type(name) is not str or (name != package and not name.startswith(package + ".")):
            continue
        if type(m) is not types.ModuleType:
            continue
        try:
            names = list(object.__getattribute__(m, "__dict__").items())
        except BaseException:
            continue
        found.extend(name + "." + k for k, v in names if v is cls and type(k) is str)
    return sorted(found)


def carry(cls, module, qualname):
    """The names class `cls`, which doesn't go by its name, carries across a restart by: see `carries`."""
    if id(cls) not in carries:
        names = [["b", where] for where in bound(cls) if first(("b", where), cls)]
        if type(module) is str and type(qualname) is str and first(("u", module, qualname), cls):
            names.append(["u", module, qualname])
        carries[id(cls)] = names
    return carries[id(cls)]


def module_named(m):
    """The name module `m` goes by, if it is the module sys.modules holds under it and the first reported there;
    else None."""
    try:
        name = object.__getattribute__(m, "__dict__").get("__name__")
    except BaseException:
        return None
    if type(name) is str and sys.modules.get(name) is m and first(("m", name), m):
        return name
    return None


def class_named(cls, module, qualname):
    """Whether class `cls` goes by its module and qualname: the object they give, walked through each object's own
    `__dict__`, never by getattr, and the first reported under them."""
    if type(module) is not str or type(qualname) is not str:
        return False
    found = sys.modules.get(module)
    try:
        for part in qualname.split("."):
            found = object.__getattribute__(found, "__dict__").get(part)
    except BaseException:
        return False
    return found is cls and first(("e", module, qualname), cls)


def described(cls, depth=0):
    """A class as [module, qualname, [its bases, described], own], as strings whatever the class says: `own` is
    None for a class that goes by its name, and otherwise its generation, its one handle and what it carries across
    a restart by (`carries`). The host keeps a class of its own for each, matching no other, and one that carries
    something keeps its class across a restart by it, as a named class does by its name (findings 90ae40b8ad52,
    9ed85ad61c06)."""
    if depth > DEPTH:
        raise ValueError("a class nested too deeply to send")
    try:
        module = type.__getattribute__(cls, "__module__")
    except BaseException:
        module = None
    qualname = type.__getattribute__(cls, "__qualname__")
    mine = None
    if not class_named(cls, module, qualname):
        mine = [generation, own_handle(cls), carry(cls, module, qualname)]
    try:
        module = str(module) if module is not None else "?"
    except BaseException:
        module = "?"
    bases = type.__getattribute__(cls, "__bases__")
    return [module, str(qualname), [described(b, depth + 1) for b in bases], mine]


# Only a value of exactly one of the plain types goes back as data (decision d1fc04a4f896): checked by identity,
# so a subclass, even of int or str, is the code's, a handle, and nothing of the code answers for it.


def enc(v, depth=0):
    if depth > DEPTH:
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
    if t is fractions.Fraction:
        return ["Q", str(v.numerator), str(v.denominator)]
    if t is complex:
        return ["c", repr(v.real), repr(v.imag)]
    if t is types.ModuleType:
        name = module_named(v)
        return ["m", name] if name is not None else ["h", generation, handle(v)]
    if isinstance(v, type) and issubclass(v, BaseException):
        return ["e", generation, handle(v), described(v)]
    return ["h", generation, handle(v)]


def dec(v, depth=0):
    if depth > DEPTH:
        raise ValueError("a value nested too deeply")
    tag = v[0]
    if tag == "n":
        return None
    if tag in ("b", "s"):
        return v[1]
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
    if tag in ("h", "e"):
        if v[1] == generation:
            return handles[v[2]]
        # Of a server that has ended: found again by name if it was a module's attribute.
        if tag == "h" and len(v) > 3:
            return getattr(module(str(v[3][0])), str(v[3][1]))
        raise Stale()
    if tag == "m":
        return module(v[1])
    if tag == "p":
        if area is None:
            raise ValueError("a path with none given")
        return plain(v[1])
    if tag == "sio":
        i = int(v[1])
        if i not in streams:
            streams[i] = io.StringIO(str(v[2]))
            streams[i].seek(int(v[3]))
        return streams[i]
    raise ValueError("an unknown value")


def module(name):
    if name != package and not name.startswith(package + "."):
        raise ImportError("only the package under test")
    return importlib.import_module(name)


def place(m):
    """Where module `m`'s file is in the tree, relative to it ("" if it has none there), never its real path:
    the host's module of that name says the same place in its own copy of the tree's layout (task
    6a80d88373a5)."""
    try:
        f = object.__getattribute__(m, "__dict__").get("__file__")
        real, root = os.path.realpath(str(f)), os.path.realpath(tree)
    except Exception:
        return ""
    if not real.startswith(root + os.sep):
        return ""
    return os.path.relpath(real, root).replace(os.sep, "/")


def raised(e):
    try:
        message = str(e)
    except BaseException:
        message = "?"
    try:
        args = enc(e.args)
    except BaseException:
        args = ["t", [["s", message]]]
    # Its attributes that are plain data or the package's objects, such as a parse error's line: its own, and
    # those its class and the package's bases declare or default, a dataclass field's or a slot's too.
    names = set()
    try:
        names.update(object.__getattribute__(e, "__dict__"))
    except BaseException:
        pass
    try:
        for cls in type.__getattribute__(type(e), "__mro__"):
            if is_builtin(cls):
                continue
            own = type.__getattribute__(cls, "__dict__")
            names.update(own.get("__annotations__", {}))
            names.update(own.get("__dataclass_fields__", {}))
            slots = own.get("__slots__", ())
            names.update([slots] if isinstance(slots, str) else slots)
            names.update(k for k, v in own.items() if not callable(v) and not isinstance(v, (property, staticmethod,
                                                                                              classmethod)))
    except BaseException:
        pass
    attrs = []
    for k in sorted(n for n in names if type(n) is str and not n.startswith("_") and n != "args")[:64]:
        try:
            v = getattr(e, k)
            if not callable(v):
                attrs.append([k, enc(v)])
        except BaseException:
            pass
    return {"raise": enc(type(e)), "message": message[:4096], "args": args, "attrs": attrs}


# The operators a test may use on the code's objects: done here, by the code (task 6a80d88373a5).
BINARY = {"add", "sub", "mul", "truediv", "floordiv", "mod", "pow", "lt", "le", "gt", "ge"}
ORDER = {"lt", "le", "gt", "ge"}
UNARY = {"neg": operator.neg, "pos": operator.pos, "abs": operator.abs}


def is_builtin(t):
    """Whether `t` is one of Python's own types taken before any of the code was imported: by identity alone,
    never `in`, a hash or `==`, which the code's metaclass would answer, nor what a class says its module is
    (findings 98fb18bf3e9c, a2c5943deae7)."""
    for b in BUILTIN_TYPES:
        if t is b:
            return True
    return False


def plain_type(v):
    """Whether `v` is of a builtin type, so nothing of the code answers for it: none of its own classes."""
    return is_builtin(type(v))


def answer(r):
    op = r["op"]
    if op == "import":
        # What the host imports is what the name gives from now on, before the code can report another under it;
        # and if the code already sent another under it, the import is refused (finding 4d05772e8e19).
        m = module(r["name"])
        if not first(("m", str(r["name"])), m):
            raise ImportError("another module was sent under the name " + str(r["name"]))
        return ["s", place(m)]
    if op == "exists":
        # Whether the package has the module, as the host's import machinery asks.
        name = str(r["name"])
        if name != package and not name.startswith(package + "."):
            return ["b", False]
        try:
            return ["b", importlib.util.find_spec(name) is not None]
        except (ImportError, ValueError):
            return ["b", False]
    if op == "binop":
        name = str(r["name"])
        if name not in BINARY:
            raise ValueError("an unknown operator")
        a, b = dec(r["a"]), dec(r["b"])
        if name in ORDER:
            # Never: the code's objects are never compared (decision d1fc04a4f896).
            raise TypeError("the code's object is never ordered")
        # The result stays the code's: a handle, whatever it is (finding c823af485f19).
        return ["h", generation, handle(getattr(operator, name)(a, b))]
    obj = dec(r["obj"])
    if op == "unop":
        name = str(r["name"])
        if name not in UNARY:
            raise ValueError("an unknown operator")
        return ["h", generation, handle(UNARY[name](obj))]
    if op == "make":
        # An exception a test makes of the code's class: made here, by its own __init__, and described as a
        # raised one is (task 6a80d88373a5).
        args = [dec(a) for a in r.get("args", [])]
        kwargs = {k: dec(x) for k, x in r.get("kwargs", {}).items()}
        if not (isinstance(obj, type) and issubclass(obj, BaseException)):
            raise TypeError("only an exception class is made so")
        made = obj(*args, **kwargs)
        if not isinstance(made, BaseException):
            raise TypeError("not an exception")
        return ["made", raised(made)]
    if op == "dir":
        return enc(sorted(str(n) for n in dir(obj)))
    if op == "getattr":
        return enc(getattr(obj, r["name"]))
    if op == "call":
        args = [dec(a) for a in r.get("args", [])]
        kwargs = {k: dec(x) for k, x in r.get("kwargs", {}).items()}
        # The host's standard output or error, when a test has redirected it to a text stream: this call's too.
        saved = sys.stdout, sys.stderr
        try:
            if "stdout" in r:
                sys.stdout = dec(r["stdout"])
            if "stderr" in r:
                sys.stderr = dec(r["stderr"])
            return enc(obj(*args, **kwargs))
        finally:
            sys.stdout, sys.stderr = saved
    if op == "list":
        return enc(list(obj))
    if op == "len":
        return enc(len(obj))
    if op == "bool":
        # Python's own truth of an object, never the code's __bool__ or __len__ answering a test's check (finding
        # c823af485f19): a builtin type's, or an object whose classes define neither, which is true.
        if plain_type(obj) or not any(
            "__bool__" in type.__getattribute__(c, "__dict__") or "__len__" in type.__getattribute__(c, "__dict__")
            for c in type.__getattribute__(type(obj), "__mro__")
            if not is_builtin(c)
        ):
            return ["b", bool(obj)]
        raise TypeError("the truth of the code's object is never the code's to say; compare a plain value")
    if op == "str":
        # A plain str, whatever __str__ gave: a subclass's copy, as str itself.
        return ["s", str.__str__(str(obj))]
    if op == "repr":
        return ["s", str.__str__(repr(obj))]
    if op == "getitem":
        return enc(obj[dec(r["key"])])
    raise ValueError("an unknown request")


def serve():
    global area, streams
    for n, line in enumerate(requests):
        answer_one(n, line)


def answer_one(n, line):
    global area, streams
    r, area = None, None
    try:
        r = json.loads(line)
        if r.get("dirs"):
            lay_out(r, n)
        reply = {"ok": answer(r)}
    except Stale:
        reply = {"gone": "an object of a server that has ended"}
    except BaseException as e:
        try:
            reply = raised(e)
        except BaseException:
            reply = {"gone": "an exception that couldn't be described"}
    given, streams = streams, {}
    try:
        reply["streams"] = [[i, s.getvalue(), s.tell()] for i, s in given.items()]
    except BaseException:
        pass
    if area is not None:
        try:
            reply["files"] = gathered()
        except BaseException:
            reply = {"gone": "the paths given couldn't be given back"}
    try:
        text = json.dumps(reply)
        # A path given says the test's own again, wherever the reply holds it.
        if area is not None and type(r) is dict:
            for shown, original in sorted(r.get("paths", []), key=lambda p: -len(str(p[0]))):
                text = text.replace(json.dumps(plain(shown))[1:-1], json.dumps(str(original))[1:-1])
            shutil.rmtree(area, ignore_errors=True)
    except BaseException:
        text = json.dumps({"gone": "a reply that couldn't be sent"})
    replies.write(text + "\n")
    replies.flush()


# The types of Python's own whose objects are plain, taken now, before any of the code is imported (finding
# 98fb18bf3e9c).
import builtins  # noqa: E402

BUILTIN_TYPES = tuple(
    [v for v in vars(builtins).values() if isinstance(v, type)]
    + [decimal.Decimal, fractions.Fraction, datetime.date, datetime.datetime, datetime.time, datetime.timedelta, types.FunctionType,
       types.BuiltinFunctionType, types.MethodType, types.ModuleType, type(None), type(NotImplemented),
       type(Ellipsis)])


def sealed():
    """The server's own functions, each made again over a copy of its names taken now, before any of the code
    is imported: so code that rebinds a name of this module, such as `is_builtin`, changes nothing the server does
    (finding 0b5b233009ef). The modules they use are still the process's, shared with the code."""
    names = dict(globals())
    # And Python's builtins as they are now, so code that replaces one, such as `type` or `str`, changes nothing
    # here either; reaching these names through frames or gc stays possible (finding a1e72cb9c89d, deferred).
    for k, v in vars(builtins).items():
        if not k.startswith("_") and k not in names:
            names[k] = v
    for k, v in list(names.items()):
        if type(v) is types.FunctionType and v.__module__ == __name__:
            names[k] = types.FunctionType(v.__code__, names, v.__name__, v.__defaults__, v.__closure__)
    return names


sealed()["serve"]()
