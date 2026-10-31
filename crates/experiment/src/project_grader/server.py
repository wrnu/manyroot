"""The grader's server: the code under test's side. It runs in the Python profile's child with the tree on its
path, and answers the host's requests about the package one JSON line at a time: standard input in, a copy of
standard output out. The code's own output goes to standard error, and its standard input is /dev/null.

Arguments: the tree, the package, and this server's generation. Every value goes back as data when it is one of
the plain types below, or a subclass of one but tuple, converted to the plain type; and otherwise as a handle, a
number standing for the object here, tagged with the generation. So the host compares only data it decoded
itself, the code's own __eq__ or __contains__ never answers for it, and a handle from a server that has ended is
refused by the next one rather than taken for another object (finding fe2549db89ee).
"""

import base64
import datetime
import decimal
import importlib
import json
import os
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


class Stale(Exception):
    """A handle of a server that has ended."""


def handle(obj):
    handles.append(obj)
    return len(handles) - 1


def described(cls, depth=0):
    """A class as [module, qualname, [its bases, described]], as strings whatever the class says."""
    if depth > DEPTH:
        raise ValueError("a class nested too deeply to send")
    try:
        module = str(type.__getattribute__(cls, "__module__"))
    except BaseException:
        module = "?"
    bases = type.__getattribute__(cls, "__bases__")
    return [module, str(type.__getattribute__(cls, "__qualname__")), [described(b, depth + 1) for b in bases]]


# Subclasses of these go back as the plain type: their own methods never answer for a test (finding d077f100ac33).
# Not tuple's, whose subclasses (named tuples) keep their attributes as handles.
PLAIN = (dict, list, set, frozenset, str, bytes, int, float, decimal.Decimal)


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
    for base in PLAIN:
        if isinstance(v, base):
            return enc(base(v), depth + 1)
    if t is types.ModuleType:
        return ["m", str(v.__name__)]
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
    raise ValueError("an unknown value")


def module(name):
    if name != package and not name.startswith(package + "."):
        raise ImportError("only the package under test")
    return importlib.import_module(name)


def raised(e):
    try:
        message = str(e)
    except BaseException:
        message = "?"
    try:
        args = enc(e.args)
    except BaseException:
        args = ["t", [["s", message]]]
    return {"raise": enc(type(e)), "message": message[:4096], "args": args}


def answer(r):
    op = r["op"]
    if op == "import":
        module(r["name"])
        return ["n"]
    obj = dec(r["obj"])
    if op == "getattr":
        return enc(getattr(obj, r["name"]))
    if op == "call":
        args = [dec(a) for a in r.get("args", [])]
        kwargs = {k: dec(x) for k, x in r.get("kwargs", {}).items()}
        return enc(obj(*args, **kwargs))
    if op == "list":
        return enc(list(obj))
    if op == "len":
        return enc(len(obj))
    if op == "bool":
        return enc(bool(obj))
    if op == "str":
        return enc(str(obj))
    if op == "repr":
        return enc(repr(obj))
    if op == "getitem":
        return enc(obj[dec(r["key"])])
    raise ValueError("an unknown request")


for line in requests:
    try:
        reply = {"ok": answer(json.loads(line))}
    except Stale:
        reply = {"gone": "an object of a server that has ended"}
    except BaseException as e:
        try:
            reply = raised(e)
        except BaseException:
            reply = {"gone": "an exception that couldn't be described"}
    try:
        text = json.dumps(reply)
    except BaseException:
        text = json.dumps({"gone": "a reply that couldn't be sent"})
    replies.write(text + "\n")
    replies.flush()
