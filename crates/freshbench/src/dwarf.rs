//! Type ground truth from DWARF: every function's return class, parameter
//! classes (in order) and local variable classes, keyed by `low_pc`.
//!
//! Locals are `DW_TAG_variable` entries in the function (lexical blocks
//! included, inlined subroutines excluded) that have a location that is not a
//! static address: the variables the compiled code actually holds somewhere.
//! Concrete out-of-line instances read names/types through
//! `DW_AT_abstract_origin`.

use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::Result;
use gimli::{AttributeValue, EndianSlice, RunTimeEndian, UnitOffset};
use object::{Object, ObjectSection};

use crate::types::{Signature, TypeClass};

type R<'a> = EndianSlice<'a, RunTimeEndian>;

/// Every unit of the file, so a reference into another compilation unit
/// (`DW_FORM_ref_addr`: LTO, partial units) can be followed.
struct Cx<'a, 'd> {
    dwarf: &'d gimli::Dwarf<R<'a>>,
    units: Vec<gimli::Unit<R<'a>>>,
}

/// Where a reference attribute points.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ref {
    /// the attribute is absent
    Absent,
    /// (unit index, offset in that unit)
    At(usize, UnitOffset),
    /// present, but in a form or place this reader cannot follow (a type
    /// signature, a supplementary file, an offset in no unit)
    Unresolved,
}

impl<'a> Cx<'a, '_> {
    fn resolve(&self, unit: usize, v: Option<AttributeValue<R<'a>>>) -> Ref {
        match v {
            None => Ref::Absent,
            Some(AttributeValue::UnitRef(o)) => Ref::At(unit, o),
            Some(AttributeValue::DebugInfoRef(off)) => self
                .units
                .iter()
                .enumerate()
                .find_map(|(i, u)| off.to_unit_offset(&u.header).map(|o| Ref::At(i, o)))
                .unwrap_or(Ref::Unresolved),
            Some(_) => Ref::Unresolved,
        }
    }

    fn class_of_type(&self, r: Ref, depth: u32) -> TypeClass {
        let (ui, off) = match r {
            // no DW_AT_type: void
            Ref::Absent => return TypeClass::Void,
            // a type exists but could not be read: unknown, never void
            Ref::Unresolved => return TypeClass::Unknown,
            Ref::At(ui, off) => (ui, off),
        };
        if depth > 16 {
            return TypeClass::Unknown;
        }
        let unit = &self.units[ui];
        let Ok(die) = unit.entry(off) else {
            return TypeClass::Unknown;
        };
        let next = || self.resolve(ui, die.attr_value(gimli::DW_AT_type));
        let size = || match die.attr_value(gimli::DW_AT_byte_size) {
            Some(AttributeValue::Udata(n)) => n as u8,
            Some(AttributeValue::Data1(n)) => n,
            Some(AttributeValue::Data2(n)) => n as u8,
            Some(AttributeValue::Data4(n)) => n as u8,
            _ => 0,
        };
        match die.tag() {
            gimli::DW_TAG_typedef
            | gimli::DW_TAG_const_type
            | gimli::DW_TAG_volatile_type
            | gimli::DW_TAG_restrict_type
            | gimli::DW_TAG_atomic_type => self.class_of_type(next(), depth + 1),
            gimli::DW_TAG_pointer_type
            | gimli::DW_TAG_reference_type
            | gimli::DW_TAG_rvalue_reference_type
            | gimli::DW_TAG_ptr_to_member_type
            | gimli::DW_TAG_subroutine_type => TypeClass::Ptr,
            gimli::DW_TAG_array_type => TypeClass::Array,
            gimli::DW_TAG_structure_type | gimli::DW_TAG_union_type | gimli::DW_TAG_class_type => {
                TypeClass::Struct
            }
            gimli::DW_TAG_enumeration_type => TypeClass::Int(size().max(4)),
            gimli::DW_TAG_base_type => {
                let enc = match die.attr_value(gimli::DW_AT_encoding) {
                    Some(AttributeValue::Encoding(e)) => e,
                    _ => return TypeClass::Unknown,
                };
                match enc {
                    gimli::DW_ATE_float => TypeClass::Float(size()),
                    gimli::DW_ATE_boolean => TypeClass::Bool,
                    _ => TypeClass::Int(size()),
                }
            }
            gimli::DW_TAG_unspecified_type => TypeClass::Void,
            _ => TypeClass::Unknown,
        }
    }

    /// The type of an entry, through abstract_origin/specification (which may
    /// lie in another unit) when it has none of its own.
    fn entry_type(&self, unit: usize, off: UnitOffset) -> Ref {
        let (mut ui, mut off) = (unit, off);
        for _ in 0..4 {
            let Ok(d) = self.units[ui].entry(off) else {
                return Ref::Unresolved;
            };
            match self.resolve(ui, d.attr_value(gimli::DW_AT_type)) {
                Ref::Absent => {}
                r => return r,
            }
            let origin = d
                .attr_value(gimli::DW_AT_abstract_origin)
                .or_else(|| d.attr_value(gimli::DW_AT_specification));
            match self.resolve(ui, origin) {
                Ref::At(u, o) => (ui, off) = (u, o),
                // no origin: the entry really has no type
                Ref::Absent => return Ref::Absent,
                Ref::Unresolved => return Ref::Unresolved,
            }
        }
        Ref::Unresolved
    }

    /// Does the variable live somewhere at run time? A location list counts
    /// only when it has at least one entry (DWARF 4 `.debug_loc` offsets and
    /// DWARF 5 `DW_FORM_loclistx` / `DW_FORM_sec_offset` alike).
    fn has_runtime_location(
        &self,
        unit: usize,
        die: &gimli::DebuggingInformationEntry<R<'a>>,
    ) -> bool {
        match die.attr_value(gimli::DW_AT_location) {
            None => false,
            // an expression that starts with DW_OP_addr (0x03) is a static
            Some(AttributeValue::Exprloc(e)) => {
                e.0.slice().first() != Some(&0x03) && !e.0.is_empty()
            }
            Some(v) => match self.dwarf.attr_locations(&self.units[unit], v) {
                Ok(Some(mut it)) => matches!(it.next(), Ok(Some(_))),
                // a location attribute this reader cannot decode: keep the variable
                Ok(None) | Err(_) => true,
            },
        }
    }

    fn collect(
        &self,
        unit: usize,
        node: gimli::EntriesTreeNode<'_, '_, R<'a>>,
        direct: bool,
        params: &mut Vec<TypeClass>,
        locals: &mut Vec<TypeClass>,
    ) -> gimli::Result<()> {
        let mut ch = node.children();
        while let Some(c) = ch.next()? {
            let d = c.entry().clone();
            match d.tag() {
                gimli::DW_TAG_formal_parameter if direct => {
                    params.push(self.class_of_type(self.entry_type(unit, d.offset()), 0))
                }
                gimli::DW_TAG_variable => {
                    if self.has_runtime_location(unit, &d) {
                        locals.push(self.class_of_type(self.entry_type(unit, d.offset()), 0));
                    }
                }
                gimli::DW_TAG_lexical_block => self.collect(unit, c, false, params, locals)?,
                _ => {} // inlined subroutines and nested types are not this function's variables
            }
        }
        Ok(())
    }
}

/// low_pc -> high_pc of every subprogram that has both.
pub fn ranges(data: &[u8]) -> Result<HashMap<u64, u64>> {
    let file = object::File::parse(data)?;
    let endian = if file.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<Cow<[u8]>, object::Error> {
        match file.section_by_name(id.name()) {
            Some(s) => s.uncompressed_data(),
            None => Ok(Cow::Borrowed(&[][..])),
        }
    };
    let owned = gimli::DwarfSections::load(load)?;
    let dwarf = owned.borrow(|s| EndianSlice::new(s, endian));
    let mut out = HashMap::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let u = unit.unit_ref(&dwarf);
        let mut cursor = unit.entries();
        while let Some(die) = cursor.next_dfs()? {
            if die.tag() != gimli::DW_TAG_subprogram {
                continue;
            }
            let Some(lv) = die.attr_value(gimli::DW_AT_low_pc) else {
                continue;
            };
            let Ok(Some(low)) = u.attr_address(lv) else {
                continue;
            };
            let high = match die.attr_value(gimli::DW_AT_high_pc) {
                Some(AttributeValue::Udata(n)) => Some(low + n),
                Some(v) => u.attr_address(v).ok().flatten(),
                None => None,
            };
            if let Some(h) = high {
                out.insert(low, h);
            }
        }
    }
    Ok(out)
}

/// low_pc -> signature for every subprogram with a low_pc in the binary.
pub fn signatures(data: &[u8]) -> Result<HashMap<u64, Signature>> {
    let file = object::File::parse(data)?;
    let endian = if file.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let load = |id: gimli::SectionId| -> Result<Cow<[u8]>, object::Error> {
        match file.section_by_name(id.name()) {
            Some(s) => s.uncompressed_data(),
            None => Ok(Cow::Borrowed(&[][..])),
        }
    };
    let owned = gimli::DwarfSections::load(load)?;
    let dwarf = owned.borrow(|s| EndianSlice::new(s, endian));
    let mut all = Vec::new();
    let mut headers = dwarf.units();
    while let Some(header) = headers.next()? {
        all.push(dwarf.unit(header)?);
    }
    let cx = Cx {
        dwarf: &dwarf,
        units: all,
    };
    let mut out = HashMap::new();
    for (ui, unit) in cx.units.iter().enumerate() {
        let u = unit.unit_ref(&dwarf);
        let mut subprograms = Vec::new();
        let mut cursor = unit.entries();
        while let Some(die) = cursor.next_dfs()? {
            if die.tag() != gimli::DW_TAG_subprogram {
                continue;
            }
            if let Some(v) = die.attr_value(gimli::DW_AT_low_pc) {
                if let Ok(Some(low)) = u.attr_address(v) {
                    subprograms.push((low, die.offset()));
                }
            }
        }
        for (low, off) in subprograms {
            let ret = cx.class_of_type(cx.entry_type(ui, off), 0);
            let (mut params, mut locals) = (Vec::new(), Vec::new());
            let mut tree = unit.entries_tree(Some(off))?;
            cx.collect(ui, tree.root()?, true, &mut params, &mut locals)?;
            locals.sort();
            out.insert(
                low,
                Signature {
                    ret: Some(ret),
                    params,
                    locals,
                },
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::*;

    /// Linux only: compile a small function with cc -g and read its signature back.
    #[cfg(target_os = "linux")]
    #[test]
    fn signature_from_elf() {
        use std::process::Command;
        if Command::new("cc").arg("--version").output().is_err() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.c");
        std::fs::write(&src, "typedef unsigned long ul;\nstruct s { int a; };\nstatic int keep;\n\
            double f(const char *p, ul n, struct s v) { int i = (int)n; char buf[8]; buf[0] = p[i]; keep = buf[0] + v.a; return (double)i; }\n\
            int main(void) { struct s v = {1}; return (int)f(\"x\", 1, v); }\n").unwrap();
        let bin = d.path().join("a.out");
        assert!(Command::new("cc")
            .args(["-g", "-O0"])
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()
            .unwrap()
            .success());
        let data = std::fs::read(&bin).unwrap();
        let sigs = signatures(&data).unwrap();
        let f = sigs.values().find(|s| s.params.len() == 3).expect("f");
        assert_eq!(f.ret, Some(TypeClass::Float(8)));
        assert_eq!(
            f.params,
            [TypeClass::Ptr, TypeClass::Int(8), TypeClass::Struct]
        );
        assert_eq!(f.locals, [TypeClass::Int(4), TypeClass::Array]);
    }
}
