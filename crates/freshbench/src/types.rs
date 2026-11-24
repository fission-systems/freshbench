//! Type recovery: the function's signature and locals against DWARF.
//!
//! Types are compared as classes, not names, because a stripped binary keeps no
//! type names and every decompiler spells types its own way:
//!
//! | class | DWARF | decompiler spelling |
//! |---|---|---|
//! | `int<N>` | base type, integer encoding, N bytes (signedness dropped) | `char` `short` `int` `long` `undefined4` `uint64_t` ... |
//! | `float<N>` | base type, float encoding | `float` `double` |
//! | `bool` | boolean encoding | `bool` `_Bool` |
//! | `ptr` | pointer / reference | any declarator with `*` |
//! | `array` | array type | declarator with `[]` |
//! | `struct` | struct / union / class | `struct X` / `union X` |
//! | `void` | no type | `void` |
//! | `enum` -> `int<N>` | enumeration of N bytes | |
//!
//! Typedefs, `const`, `volatile`, `restrict` and `_Atomic` are looked through.
//! A decompiler type name that maps to no class (a name the tool invented) is
//! `unknown` and never matches.
//!
//! Per function:
//! * signature: return class, and parameter classes by position, plus arity;
//! * locals: the multiset of local variable classes (DWARF `DW_TAG_variable`
//!   with a stack or register location inside the function, outside inlined
//!   subroutines) against the output's local declarations.
//!
//! `perfect` = right arity, every parameter class right, return class right.
//! Locals are reported (coverage, precision) but not part of `perfect`: at
//! -O2 most locals live in registers and have no DWARF location the output
//! could be held to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::csyntax::text;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypeClass {
    Int(u8),
    Float(u8),
    Bool,
    Ptr,
    Array,
    Struct,
    Void,
    Unknown,
}

impl std::fmt::Display for TypeClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeClass::Int(n) => write!(f, "int{n}"),
            TypeClass::Float(n) => write!(f, "float{n}"),
            TypeClass::Bool => write!(f, "bool"),
            TypeClass::Ptr => write!(f, "ptr"),
            TypeClass::Array => write!(f, "array"),
            TypeClass::Struct => write!(f, "struct"),
            TypeClass::Void => write!(f, "void"),
            TypeClass::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub ret: Option<TypeClass>,
    pub params: Vec<TypeClass>,
    pub locals: Vec<TypeClass>,
}

/// Data model of the target: pointer and `long` width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataModel {
    pub long: u8,
    pub ptr: u8,
}

impl DataModel {
    pub fn for_arch(arch: &str) -> DataModel {
        match arch {
            "i686" | "armv7" => DataModel { long: 4, ptr: 4 },
            _ => DataModel { long: 8, ptr: 8 },
        }
    }
}

/// Class of a C type name as decompilers print it (no declarator).
pub fn class_of_name(name: &str, dm: DataModel) -> TypeClass {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| {
            !matches!(
                *w,
                "const"
                    | "volatile"
                    | "restrict"
                    | "__restrict"
                    | "static"
                    | "register"
                    | "extern"
                    | "_Atomic"
                    | "signed"
                    | "unsigned"
            )
        })
        .collect();
    let joined = words.join(" ");
    let unsigned_only = name
        .split_whitespace()
        .any(|w| w == "unsigned" || w == "signed")
        && words.is_empty();
    if unsigned_only {
        return TypeClass::Int(4);
    }
    if words
        .first()
        .is_some_and(|w| *w == "struct" || *w == "union" || *w == "class")
    {
        return TypeClass::Struct;
    }
    if words.first().is_some_and(|w| *w == "enum") {
        return TypeClass::Int(4);
    }
    let i = |n: u8| TypeClass::Int(n);
    match joined.as_str() {
        "void" => TypeClass::Void,
        "bool" | "_Bool" | "boolean" => TypeClass::Bool,
        "char" | "uchar" | "byte" | "int8_t" | "uint8_t" | "undefined1" | "undefined" | "u8"
        | "i8" | "s8" | "BYTE" | "CHAR" | "UCHAR" => i(1),
        "short" | "short int" | "ushort" | "int16_t" | "uint16_t" | "undefined2" | "word"
        | "u16" | "i16" | "s16" | "WORD" | "wchar16" => i(2),
        "int" | "uint" | "int32_t" | "uint32_t" | "undefined4" | "dword" | "u32" | "i32"
        | "s32" | "DWORD" | "UINT" | "INT" | "wchar_t" | "wchar32" => i(4),
        "long long" | "long long int" | "ulonglong" | "longlong" | "int64_t" | "uint64_t"
        | "undefined8" | "qword" | "u64" | "i64" | "s64" | "QWORD" | "__int64" => i(8),
        "long" | "long int" | "ulong" | "LONG" | "ULONG" => i(dm.long),
        "size_t" | "ssize_t" | "intptr_t" | "uintptr_t" | "ptrdiff_t" | "off_t" => i(dm.ptr),
        "__int128" | "int128_t" | "uint128_t" | "undefined16" => i(16),
        "float" | "float4" => TypeClass::Float(4),
        "double" | "float8" => TypeClass::Float(8),
        "long double" | "float10" => TypeClass::Float(16),
        _ => TypeClass::Unknown,
    }
}

/// Type text and declarator of a declaration/parameter -> class (pointer/array from the declarator).
fn class_of_decl(ty: Node, decl: Option<Node>, src: &str, dm: DataModel) -> TypeClass {
    let mut d = decl;
    while let Some(n) = d {
        match n.kind() {
            "pointer_declarator" | "abstract_pointer_declarator" | "function_declarator" => {
                return TypeClass::Ptr
            }
            "array_declarator" | "abstract_array_declarator" => return TypeClass::Array,
            "init_declarator" | "parenthesized_declarator" | "attributed_declarator" => {
                d = n
                    .child_by_field_name("declarator")
                    .or_else(|| n.named_child(0));
            }
            _ => break,
        }
    }
    let t = text(ty, src);
    if matches!(ty.kind(), "struct_specifier" | "union_specifier") {
        return TypeClass::Struct;
    }
    if ty.kind() == "enum_specifier" {
        return TypeClass::Int(4);
    }
    class_of_name(t, dm)
}

/// Signature and local classes of a function definition in C text.
pub fn from_source(def: Node, src: &str, dm: DataModel) -> Signature {
    let mut sig = Signature::default();
    if let Some(ty) = def.child_by_field_name("type") {
        // a pointer return lives in the declarator: `T *f(...)`
        let mut d = def.child_by_field_name("declarator");
        let mut ptr = false;
        while let Some(n) = d {
            if n.kind() == "pointer_declarator" {
                ptr = true;
                break;
            }
            if n.kind() == "function_declarator" {
                break;
            }
            d = n.child_by_field_name("declarator");
        }
        sig.ret = Some(if ptr {
            TypeClass::Ptr
        } else {
            class_of_decl(ty, None, src, dm)
        });
    }
    // parameters of the function_declarator
    let mut d = def.child_by_field_name("declarator");
    while let Some(n) = d {
        if n.kind() == "function_declarator" {
            if let Some(pl) = n.child_by_field_name("parameters") {
                let mut c = pl.walk();
                for p in pl.named_children(&mut c) {
                    if p.kind() != "parameter_declaration" {
                        continue; // `...`
                    }
                    let Some(ty) = p.child_by_field_name("type") else {
                        continue;
                    };
                    let cls = class_of_decl(ty, p.child_by_field_name("declarator"), src, dm);
                    if cls == TypeClass::Void && p.child_by_field_name("declarator").is_none() {
                        continue; // f(void)
                    }
                    sig.params.push(cls);
                }
            }
            break;
        }
        d = n.child_by_field_name("declarator");
    }
    // local declarations in the body (not nested functions)
    if let Some(body) = def.child_by_field_name("body") {
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "declaration" {
                if let Some(ty) = n.child_by_field_name("type") {
                    let is_static = {
                        let mut c = n.walk();
                        let r = n.children(&mut c).any(|ch| {
                            ch.kind() == "storage_class_specifier" && text(ch, src) == "static"
                        });
                        r
                    };
                    if !is_static {
                        let mut c = n.walk();
                        for dch in n.children_by_field_name("declarator", &mut c) {
                            sig.locals.push(class_of_decl(ty, Some(dch), src, dm));
                        }
                    }
                }
                continue;
            }
            let mut c = n.walk();
            let kids: Vec<_> = n.named_children(&mut c).collect();
            stack.extend(kids.into_iter().rev());
        }
    }
    sig.locals.sort();
    sig
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TypeScore {
    pub params_truth: u32,
    pub params_output: u32,
    pub params_right: u32,
    pub return_right: bool,
    pub locals_truth: u32,
    pub locals_output: u32,
    pub locals_matched: u32,
    /// arity, every parameter class and the return class right
    pub perfect: bool,
}

pub fn compare(truth: &Signature, out: &Signature) -> TypeScore {
    let params_right = truth
        .params
        .iter()
        .zip(&out.params)
        .filter(|(a, b)| a == b && **a != TypeClass::Unknown)
        .count() as u32;
    let return_right =
        truth.ret.is_some() && truth.ret == out.ret && truth.ret != Some(TypeClass::Unknown);
    let mut t: BTreeMap<&TypeClass, u32> = BTreeMap::new();
    for c in &truth.locals {
        *t.entry(c).or_default() += 1;
    }
    let mut o: BTreeMap<&TypeClass, u32> = BTreeMap::new();
    for c in &out.locals {
        if *c != TypeClass::Unknown {
            *o.entry(c).or_default() += 1;
        }
    }
    let locals_matched = t
        .iter()
        .map(|(k, n)| (*n).min(o.get(k).copied().unwrap_or(0)))
        .sum();
    let arity = truth.params.len() == out.params.len();
    TypeScore {
        params_truth: truth.params.len() as u32,
        params_output: out.params.len() as u32,
        params_right,
        return_right,
        locals_truth: truth.locals.len() as u32,
        locals_output: out.locals.len() as u32,
        locals_matched,
        perfect: arity && params_right == truth.params.len() as u32 && return_right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csyntax::{output_function, parse};

    const LP64: DataModel = DataModel { long: 8, ptr: 8 };

    fn sig(code: &str, dm: DataModel) -> Signature {
        let t = parse(code).unwrap();
        from_source(output_function(&t, code, "f").unwrap(), code, dm)
    }

    #[test]
    fn names_to_classes() {
        assert_eq!(class_of_name("unsigned long", LP64), TypeClass::Int(8));
        assert_eq!(
            class_of_name("unsigned long", DataModel::for_arch("armv7")),
            TypeClass::Int(4)
        );
        assert_eq!(class_of_name("const char", LP64), TypeClass::Int(1));
        assert_eq!(class_of_name("undefined4", LP64), TypeClass::Int(4));
        assert_eq!(class_of_name("unsigned", LP64), TypeClass::Int(4));
        assert_eq!(class_of_name("struct foo", LP64), TypeClass::Struct);
        assert_eq!(class_of_name("double", LP64), TypeClass::Float(8));
        assert_eq!(class_of_name("MyTypedef", LP64), TypeClass::Unknown);
    }

    #[test]
    fn source_signature_and_locals() {
        let s = sig("static unsigned long *f(const char *s, int n, struct st v, ...)\n{\n  int i = 0;\n  double d, e[4];\n  static int k;\n  char *p;\n  return 0;\n}\n", LP64);
        assert_eq!(s.ret, Some(TypeClass::Ptr));
        assert_eq!(
            s.params,
            [TypeClass::Ptr, TypeClass::Int(4), TypeClass::Struct]
        );
        assert_eq!(
            s.locals,
            [
                TypeClass::Int(4),
                TypeClass::Float(8),
                TypeClass::Ptr,
                TypeClass::Array
            ]
        );
        assert_eq!(sig("void f(void) {}", LP64).params, Vec::<TypeClass>::new());
    }

    #[test]
    fn scoring() {
        let truth = sig("long f(char *a, int b) { int x; char *y; return 0; }", LP64);
        let good = sig(
            "undefined8 f(byte *param_1, uint param_2) { int iVar1; long lVar2; return 0; }",
            LP64,
        );
        let s = compare(&truth, &good);
        assert!(s.perfect);
        assert_eq!(
            (
                s.params_right,
                s.locals_truth,
                s.locals_output,
                s.locals_matched
            ),
            (2, 2, 2, 1)
        );
        let bad = sig("void f(long a) { return; }", LP64);
        let b = compare(&truth, &bad);
        assert!(!b.perfect && !b.return_right);
        assert_eq!(b.params_right, 0);
        // unknown never matches
        let unk = sig("Foo f(Foo a, Foo b) { return 0; }", LP64);
        let u = compare(
            &Signature {
                ret: Some(TypeClass::Unknown),
                params: vec![TypeClass::Unknown; 2],
                locals: vec![],
            },
            &unk,
        );
        assert!(!u.perfect);
    }
}
