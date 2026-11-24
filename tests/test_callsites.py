from collections import Counter

from freshbench.callsites import clean_symbol, compare, normalize_callee, parse_objdump, symbol_table

X86 = """
a.out:	file format elf64-x86-64

Disassembly of section .plt:

0000000000401020 <puts@plt>:
  401020:      	jmpq	*0x2fe2(%rip)           # 0x404008 <puts@GLIBC_2.2.5>

Disassembly of section .text:

0000000000401136 <helper>:
  401136:      	pushq	%rbp
  401137:      	callq	0x401020 <puts@plt>
  40113c:      	jne	0x401150 <helper+0x1a>
  40113e:      	callq	*%rax
  401140:      	jmp	0x401136 <helper>
  401142:      	popq	%rbp
  401143:      	retq

0000000000401150 <main>:
  401150:      	callq	0x401136 <helper>
  401155:      	callq	0x401136 <helper>
  40115a:      	calll	0x401200 <__x86.get_pc_thunk.bx>
  40115f:      	callq	0x401020 <puts@plt>
  401164:      	jmp	0x401136 <helper>
"""

A64 = """
0000000000000750 <compute>:
     750:      	stp	x29, x30, [sp, #-0x10]!
     754:      	bl	0x600 <memcpy@plt>
     758:      	b.ne	0x770 <compute+0x20>
     75c:      	blr	x8
     760:      	b	0x800 <tailed>
"""

ARM = """
00010400 <thumbfn>:
   10400:      	push	{r7, lr}
   10402:      	bl	0x10300 <printf@plt>
   10406:      	blx	r3
   10408:      	bl	0x10500 <__aeabi_idiv>
   1040c:      	b.w	0x10600 <other>
"""


def test_x86_64():
    f = parse_objdump(X86)
    assert dict(f[0x401136].calls) == {"puts": 1}  # self-jump is a loop, indirect not counted
    assert dict(f[0x401150].calls) == {"helper": 3, "puts": 1}  # 2 calls + 1 tail call; pc thunk excluded
    assert f[0x401150].name == "main"


def test_aarch64_and_arm():
    assert dict(parse_objdump(A64)[0x750].calls) == {"memcpy": 1, "tailed": 1}
    assert dict(parse_objdump(ARM)[0x10400].calls) == {"printf": 1, "other": 1}


def test_clean_symbol():
    assert clean_symbol("puts@plt") == "puts"
    assert clean_symbol("memcpy@GLIBC_2.14") == "memcpy"


def test_normalize_callee_styles():
    a2n = symbol_table(parse_objdump(X86))
    for name in ("FUN_00401136", "sub_401136", "func_0x401136", "fcn.00401136", "helper"):
        assert normalize_callee(name, a2n) == "helper", name
    assert normalize_callee("sub_999999", a2n) == "sub_999999"
    assert normalize_callee("sub_501136", a2n, delta=0x100000) == "helper"


def test_compare():
    f = parse_objdump(X86)
    a2n = symbol_table(f)
    out = Counter({"sub_401136": 2, "puts": 2, "__stack_chk_fail": 1})
    ag = compare(f[0x401150].calls, out, a2n)
    assert ag.as_dict() == {"binary_calls": 4, "output_calls": 4, "matched": 3, "missing": 1,
                            "excess": 1, "exact": False}
    assert compare(f[0x401136].calls, Counter({"puts": 1}), a2n).exact
