"""Python binding over the lilook C ABI.

Mirrors the Julia binding; both go through the same header, which is also what
Swift consumes. Nothing here knows about the intent vocabulary beyond passing
JSON through, so new intents need no binding changes.
"""
import ctypes
import json
import os


def _load(path=None):
    path = path or os.environ.get("LILOOK_LIB", "liblilook_ffi.so")
    lib = ctypes.CDLL(path)
    c, p, v, s = ctypes.c_char_p, ctypes.POINTER(ctypes.c_char), ctypes.c_void_p, ctypes.c_size_t
    lib.lilook_doc_new.argtypes, lib.lilook_doc_new.restype = [c], v
    lib.lilook_doc_free.argtypes = [v]
    lib.lilook_string_free.argtypes = [v]
    lib.lilook_doc_text.argtypes, lib.lilook_doc_text.restype = [v], v
    lib.lilook_doc_calls_json.argtypes, lib.lilook_doc_calls_json.restype = [v], v
    lib.lilook_doc_begin.argtypes = [v, c]
    lib.lilook_doc_commit.argtypes = [v]
    lib.lilook_doc_apply_json.argtypes = [v, c, ctypes.POINTER(v)]
    lib.lilook_doc_apply_json.restype = ctypes.c_int
    lib.lilook_doc_undo.argtypes, lib.lilook_doc_undo.restype = [v], ctypes.c_int
    lib.lilook_doc_redo.argtypes, lib.lilook_doc_redo.restype = [v], ctypes.c_int
    lib.lilook_doc_undo_depth.argtypes, lib.lilook_doc_undo_depth.restype = [v], s
    return lib


_LIB = None


def _lib():
    global _LIB
    if _LIB is None:
        _LIB = _load()
    return _LIB


def _take(ptr):
    """Copy an owned C string out and release it."""
    if not ptr:
        return None
    val = ctypes.cast(ptr, ctypes.c_char_p).value.decode()
    _lib().lilook_string_free(ptr)
    return val


class Document:
    def __init__(self, text):
        self._h = _lib().lilook_doc_new(text.encode())
        if not self._h:
            raise ValueError("could not create document")

    def __del__(self):
        if getattr(self, "_h", None):
            _lib().lilook_doc_free(self._h)
            self._h = None

    @property
    def text(self):
        return _take(_lib().lilook_doc_text(self._h))

    @property
    def calls(self):
        return json.loads(_take(_lib().lilook_doc_calls_json(self._h)))

    def begin(self, label):
        _lib().lilook_doc_begin(self._h, label.encode())

    def commit(self):
        _lib().lilook_doc_commit(self._h)

    def apply(self, **intent):
        err = ctypes.c_void_p()
        rc = _lib().lilook_doc_apply_json(
            self._h, json.dumps(intent).encode(), ctypes.byref(err))
        if rc != 0:
            raise RuntimeError(_take(err) or f"apply failed ({rc})")

    def set_arg(self, node, param, value):
        self.apply(op="set-named-arg", node=node, param=param, value=value)

    def add_arg(self, node, param, value):
        self.apply(op="insert-named-arg", node=node, param=param, value=value)

    def undo(self):
        return bool(_lib().lilook_doc_undo(self._h))

    def redo(self):
        return bool(_lib().lilook_doc_redo(self._h))

    @property
    def undo_depth(self):
        return _lib().lilook_doc_undo_depth(self._h)
