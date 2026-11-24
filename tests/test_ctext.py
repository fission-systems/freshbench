from freshbench.ctext import extract_function, features, int_value, tokens


def test_comments_strings_and_preprocessor_are_not_code():
    f = features('/* goto x; */ // return 1;\n#define X(a) foo(a)\nchar *s = "if (a && b) goto c;";')
    assert (f.goto, f.returns, f.ifs, f.logical_and, f.n_calls) == (0, 0, 0, 0, 0)
    assert f.strings['"if (a && b) goto c;"'] == 1


def test_control_counts():
    src = """
    int f(int a, int b) {
        if (a && b || !a) goto out;
        for (int i = 0; i < a; i++) { a += i; }
        while (b) b--;
        do { a++; } while (a < 10);
        switch (a) { case 1: return 1; case 2: break; default: return 0; }
        b = a ? 1 : 2;
    out:
        return b;
    }"""
    f = features(src, own_name="f")
    assert f.goto == 1
    assert f.labels == 1
    assert f.returns == 3
    assert f.ifs == 1
    assert f.loops == 3  # for, while, do-while once
    assert f.switches == 1
    assert f.cases == 3
    assert (f.logical_and, f.logical_or, f.ternary) == (1, 1, 1)


def test_calls_exclude_keywords_declarations_members():
    code = """extern long syscall(long n, ...);
    uint g(long p) {
        x = (int)foo(p) + sizeof(int);
        s->cb(p);
        if (!bar(1)) return baz(2, qux(3));
        return 0;
    }"""
    f = features(code, own_name="g")
    assert dict(f.calls) == {"foo": 1, "bar": 1, "baz": 1, "qux": 1}


def test_ternary_colon_is_not_a_label():
    assert features("x = c ? a : b;").labels == 0


def test_int_constants_and_negation():
    f = features("x = 0x10 + 010 - 3; y = -7; z = 1.5; w = 42u;")
    assert f.int_constants[16] == 1
    assert f.int_constants[8] == 1
    assert f.int_constants[3] == 1  # binary minus, not a negative constant
    assert f.int_constants[-7] == 1
    assert f.int_constants[42] == 1
    assert 1 not in f.int_constants


def test_int_value():
    assert int_value("0xffUL") == 255
    assert int_value("0b101") == 5
    assert int_value("1e3") is None
    assert int_value("077") == 63


def test_tokens_skip_whitespace():
    assert [t.text for t in tokens("a  ->b")] == ["a", "->", "b"]


TU = """
#include <stdio.h>
static int helper(int x);
static int
helper(int x)
{
    if (x) { return x; }
    return 0;
}
int main(void) { return helper(1); }
struct s { int (*helper)(int); };
"""


def test_extract_function_finds_definition_not_prototype():
    body = extract_function(TU, "helper")
    assert body.startswith("static int\nhelper(int x)")
    assert body.rstrip().endswith("}")
    assert "main" not in body


def test_extract_function_missing_or_ambiguous():
    assert extract_function(TU, "nope") is None
    two = "int f(void) { return 1; }\nint f(void) { return 2; }\n"
    assert extract_function(two, "f") is None
