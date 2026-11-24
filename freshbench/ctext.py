"""Lexical features of C text: decompiler output and source functions alike.

One tokenizer is used for both sides so a difference in a count is a
difference in the text, not in the parsers. It is deliberately lexical (no
grammar): it never fails on the partial or non-standard C that decompilers
print, and its result does not depend on an optional parser being installed.

What it counts is exactly what its name says. `calls` is every identifier
directly followed by "(" that is not a keyword, a declaration of the
function itself, or a type in a cast/sizeof; a function-like macro in the
source also counts, which is why call-site comparisons against the binary
(callsites.py) are preferred where binaries are available.
"""
from __future__ import annotations

import re
from collections import Counter
from dataclasses import dataclass, field

KEYWORDS = frozenset(
    """auto break case char const continue default do double else enum extern float for goto
    if inline int long register restrict return short signed sizeof static struct switch typedef
    union unsigned void volatile while _Alignas _Alignof _Atomic _Bool _Complex _Generic
    _Imaginary _Noreturn _Static_assert _Thread_local __attribute__ __asm__ asm __typeof__
    typeof __extension__ __inline __inline__ __restrict __restrict__ __volatile__ __builtin_va_arg
    __declspec __cdecl __stdcall __fastcall __thiscall alignof alignas bool true false nullptr
    static_assert thread_local""".split()
)

_TOKEN = re.compile(
    r"""
    (?P<ws>\s+)
  | (?P<lcomment>//[^\n]*)
  | (?P<bcomment>/\*.*?(\*/|\Z))
  | (?P<pp>^[ \t]*\#[^\n]*(?:\\\n[^\n]*)*)
  | (?P<string>L?"(?:\\.|[^"\\\n])*")
  | (?P<char>L?'(?:\\.|[^'\\\n])*')
  | (?P<number>(?:0[xX][0-9a-fA-F]+|0[bB][01]+|\d+\.?\d*(?:[eE][+-]?\d+)?|\.\d+(?:[eE][+-]?\d+)?)[uUlLfF]*)
  | (?P<ident>[A-Za-z_$][A-Za-z0-9_$]*)
  | (?P<op>->|\+\+|--|<<=|>>=|<<|>>|<=|>=|==|!=|&&|\|\||[-+*/%&|^]=|::|\.\.\.|[{}()\[\];,.?:~!<>=+\-*/%&|^#@\\])
  | (?P<other>.)
    """,
    re.S | re.M | re.X,
)


@dataclass(frozen=True)
class Token:
    kind: str  # string, char, number, ident, op, other
    text: str


def tokens(text: str) -> list[Token]:
    """Tokens of `text` without whitespace, comments and preprocessor lines."""
    out: list[Token] = []
    for m in _TOKEN.finditer(text):
        kind = m.lastgroup
        if kind in ("ws", "lcomment", "bcomment", "pp"):
            continue
        out.append(Token(kind, m.group()))
    return out


def int_value(literal: str) -> int | None:
    """Value of an integer literal (suffixes ignored); None for floating point."""
    s = literal.rstrip("uUlL")
    try:
        if s[:2] in ("0x", "0X"):
            return int(s, 16)
        if s[:2] in ("0b", "0B"):
            return int(s[2:], 2)
        if any(c in s for c in ".eEfF"):
            return None
        if len(s) > 1 and s.startswith("0"):
            return int(s, 8)
        return int(s)
    except ValueError:
        return None


@dataclass
class Features:
    goto: int = 0
    labels: int = 0
    returns: int = 0
    ifs: int = 0
    loops: int = 0
    switches: int = 0
    cases: int = 0
    logical_and: int = 0
    logical_or: int = 0
    ternary: int = 0
    calls: Counter = field(default_factory=Counter)
    int_constants: Counter = field(default_factory=Counter)
    strings: Counter = field(default_factory=Counter)

    @property
    def n_calls(self) -> int:
        return sum(self.calls.values())

    def as_dict(self) -> dict:
        return {
            "goto": self.goto, "labels": self.labels, "returns": self.returns, "ifs": self.ifs,
            "loops": self.loops, "switches": self.switches, "cases": self.cases,
            "logical_and": self.logical_and, "logical_or": self.logical_or,
            "ternary": self.ternary, "calls": self.n_calls,
        }


def features(text: str, own_name: str | None = None) -> Features:
    """Counts over one function's text (or several functions' text)."""
    toks = tokens(text)
    f = Features()
    n = len(toks)
    for i, t in enumerate(toks):
        nxt = toks[i + 1] if i + 1 < n else None
        prev = toks[i - 1] if i > 0 else None
        if t.kind == "ident":
            w = t.text
            if w == "goto":
                f.goto += 1
            elif w == "return":
                f.returns += 1
            elif w == "if":
                f.ifs += 1
            elif w in ("for", "while"):
                # `do { } while (c);` counts once: the `do`
                if w == "while" and _closes_do(toks, i):
                    continue
                f.loops += 1
            elif w == "do":
                f.loops += 1
            elif w == "switch":
                f.switches += 1
            elif w in ("case", "default") and _is_case_label(toks, i):
                f.cases += 1
            elif nxt is not None and nxt.text == ":" and _is_label(toks, i):
                f.labels += 1
            elif nxt is not None and nxt.text == "(" and w not in KEYWORDS and w != own_name:
                if prev is not None and prev.text in (".", "->"):
                    pass  # x.f(...) / x->f(...): call through a member, name unknown
                elif _is_cast_or_decl(toks, i):
                    pass
                else:
                    f.calls[w] += 1
        elif t.kind == "op":
            if t.text == "&&":
                f.logical_and += 1
            elif t.text == "||":
                f.logical_or += 1
            elif t.text == "?":
                f.ternary += 1
        elif t.kind == "number":
            v = int_value(t.text)
            if v is not None:
                neg = prev is not None and prev.text == "-" and _unary_context(toks, i - 1)
                f.int_constants[-v if neg else v] += 1
        elif t.kind == "string":
            f.strings[t.text] += 1
    return f


def _closes_do(toks: list[Token], i: int) -> bool:
    return i > 0 and toks[i - 1].text == "}" and _matching_open_preceded_by(toks, i - 1, "do")


def _matching_open_preceded_by(toks: list[Token], close: int, word: str) -> bool:
    depth = 0
    for j in range(close, -1, -1):
        if toks[j].text == "}":
            depth += 1
        elif toks[j].text == "{":
            depth -= 1
            if depth == 0:
                return j > 0 and toks[j - 1].text == word
    return False


def _is_case_label(toks: list[Token], i: int) -> bool:
    # `default:` or `case X:`; `default` elsewhere is not a label in C
    if toks[i].text == "default":
        return i + 1 < len(toks) and toks[i + 1].text == ":"
    return True


def _is_label(toks: list[Token], i: int) -> bool:
    # `name:` at statement start, not `a ? b : c` and not a bit-field
    if i == 0:
        return True
    p = toks[i - 1].text
    if p not in (";", "{", "}", ":", ")"):
        return False
    if p == ")":
        return False  # `(cond) ? x : y` style; labels never follow ')'
    # a `?` before the label at the same depth means ternary
    depth = 0
    for j in range(i - 1, -1, -1):
        s = toks[j].text
        if s in (";", "{", "}"):
            return True
        if s == "?" and depth == 0:
            return False
    return True


def _is_cast_or_decl(toks: list[Token], i: int) -> bool:
    """`name(` that is not a call: a prototype/definition inside the text.

    A call is preceded by an operator, `(`, `,`, `return`, or starts a
    statement. A definition or prototype is preceded by a type word or `*`
    that itself follows a statement boundary - e.g. `int foo(` / `char *foo(`.
    """
    if i == 0:
        return False
    j = i - 1
    while j >= 0 and toks[j].text == "*":
        j -= 1
    if j < 0:
        return False
    p = toks[j]
    if p.kind != "ident" or p.text in ("return", "case", "else", "do", "goto", "sizeof"):
        return False
    # p is an identifier right before our name: `T name(` - a declaration,
    # unless p is a macro-like word on its own; treat ident-ident as decl.
    return True


def _unary_context(toks: list[Token], minus: int) -> bool:
    if minus == 0:
        return True
    p = toks[minus - 1]
    return p.kind == "op" and p.text not in (")", "]") or p.text in ("return", "case")


def definitions(tu_text: str) -> dict[str, str | None]:
    """Every top-level function definition in a TU: name -> text (None if defined twice).

    One pass over the TU; `extract_function` for many names of one TU is this
    plus a lookup.
    """
    out: dict[str, str | None] = {}
    depth = 0
    skip_until = -1
    for m in _TOKEN.finditer(tu_text):
        if m.start() < skip_until:
            continue
        kind, s = m.lastgroup, m.group()
        if kind in ("ws", "lcomment", "bcomment", "pp", "string", "char"):
            continue
        if s == "{":
            depth += 1
            continue
        if s == "}":
            depth -= 1
            continue
        if depth != 0 or kind != "ident" or s in KEYWORDS:
            continue
        end = _definition_body(tu_text, m.end())
        if end is None:
            continue
        start = _decl_start(tu_text, m.start())
        out[s] = None if s in out else tu_text[start:end]
        skip_until = end
    return out


def _decl_start(text: str, name_pos: int) -> int:
    start = text.rfind("\n", 0, name_pos) + 1
    # include a return type on the previous line(s): back up over lines that
    # do not end a statement or block
    while start > 0:
        prev_end = start - 1
        prev_start = text.rfind("\n", 0, prev_end) + 1
        line = text[prev_start:prev_end].strip()
        if not line or line.endswith((";", "}", "{")) or line.startswith("#"):
            break
        start = prev_start
    return start


def extract_function(tu_text: str, name: str) -> str | None:
    """Text of the top-level definition of `name` in a translation unit.

    From the start of the declaration (return type lines included) to the
    matching `}`. None when there is no definition or more than one (an
    ambiguous source is not guessed at).
    """
    return definitions(tu_text).get(name)


def _definition_body(text: str, pos: int) -> int | None:
    """End offset of the body when `pos` follows a function name being defined."""
    rest = _TOKEN.finditer(text, pos)
    seen_paren = False
    depth = 0
    for m in rest:
        kind, s = m.lastgroup, m.group()
        if kind in ("ws", "lcomment", "bcomment"):
            continue
        if not seen_paren:
            if s != "(":
                return None
            seen_paren = True
            depth = 1
            continue
        if depth > 0:
            if s == "(":
                depth += 1
            elif s == ")":
                depth -= 1
            continue
        # after the parameter list: `{` starts the body, `;`/`,`/`=`/`)` mean not a definition
        if s == "{":
            return _match_brace(text, m.start())
        if s in (";", ",", "=", ")"):
            return None
    return None


def _match_brace(text: str, open_pos: int) -> int | None:
    depth = 0
    for m in _TOKEN.finditer(text, open_pos):
        kind, s = m.lastgroup, m.group()
        if kind in ("ws", "lcomment", "bcomment", "pp", "string", "char"):
            continue
        if s == "{":
            depth += 1
        elif s == "}":
            depth -= 1
            if depth == 0:
                return m.end()
    return None
