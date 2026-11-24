"""A minimal pure-Python stand-in for the few numpy calls tests/mkdst.py makes
(numpy is not installed here). 1-D integer/float arrays as Python lists; the
2-D results of stack() are only used through reshape(-1).tolist() and
linalg.lstsq(), which are special-cased."""
import math
from . import linalg

int8 = "int8"; int32 = "int32"; int64 = "int64"; uint8 = "uint8"; float64 = "float64"

def _conv(v, dtype):
    if dtype == float64:
        return float(v)
    return int(v)

class ndarray:
    __slots__ = ("v", "dtype")
    def __init__(self, v, dtype=int64):
        self.v = v
        self.dtype = dtype
    def __len__(self):
        return len(self.v)
    def __iter__(self):
        return iter(self.v)
    def tolist(self):
        return list(self.v)
    def astype(self, dtype):
        if dtype == float64:
            return ndarray([float(x) for x in self.v], dtype)
        return ndarray([int(x) for x in self.v], dtype)
    def __getitem__(self, k):
        if isinstance(k, slice):
            return ndarray(self.v[k], self.dtype)
        if isinstance(k, ndarray):
            t = self.v
            return ndarray([t[i] for i in k.v], self.dtype)
        return self.v[k]
    def __setitem__(self, k, val):
        if isinstance(k, slice):
            idx = range(*k.indices(len(self.v)))
            for i in idx:
                self.v[i] = val
        else:
            self.v[k] = val
    def _bin(self, o, f):
        if isinstance(o, ndarray):
            return ndarray([f(a, b) for a, b in zip(self.v, o.v)], self.dtype)
        return ndarray([f(a, o) for a in self.v], self.dtype)
    def __add__(self, o): return self._bin(o, lambda a, b: a + b)
    def __sub__(self, o): return self._bin(o, lambda a, b: a - b)
    def __mul__(self, o): return self._bin(o, lambda a, b: a * b)
    __rmul__ = __mul__
    def __and__(self, o): return self._bin(o, lambda a, b: a & b)
    def __xor__(self, o): return self._bin(o, lambda a, b: a ^ b)
    def __rshift__(self, o): return self._bin(o, lambda a, b: a >> b)
    def __lt__(self, o): return self._bin(o, lambda a, b: 1 if a < b else 0)
    def reshape(self, n):
        return self

class _stacked:
    """stack(axis=1): rows are frames, columns the inputs."""
    def __init__(self, cols):
        self.cols = cols
    def reshape(self, n):
        out = []
        for row in zip(*[c.v for c in self.cols]):
            out.extend(row)
        return ndarray(out)

def array(v, dtype=int64):
    return ndarray([_conv(x, dtype) for x in v], dtype)

def concatenate(parts):
    out = []
    for p in parts:
        out.extend(p.v)
    return ndarray(out, parts[0].dtype)

def convolve(x, h):
    xv = x.v; hv = h.v
    n = len(xv) + len(hv) - 1
    y = [0] * n
    for m, c in enumerate(hv):
        if c == 0:
            continue
        seg = y[m:m + len(xv)]
        y[m:m + len(xv)] = [a + c * b for a, b in zip(seg, xv)]
    return ndarray(y)

def stack(arrays, axis=0):
    assert axis == 1
    return _stacked(list(arrays))

def minimum(a, b):
    return a._bin(b, lambda x, y: x if x < y else y)

def abs(a):
    return ndarray([x if x >= 0 else -x for x in a.v], a.dtype)

def frombuffer(b, dtype=uint8):
    return ndarray(list(bytes(b)), dtype)

def unpackbits(a):
    out = []
    for byte in a.v:
        out.extend(((byte >> (7 - i)) & 1) for i in range(8))
    return ndarray(out, uint8)

def full(n, val, dtype=int64):
    return ndarray([val] * n, dtype)
