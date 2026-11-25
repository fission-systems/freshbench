//! What a decompiler says about its own output, as data: which binary
//! instructions each output line came from (`line_mappings`), the variables it
//! recovered with their ABI argument position, stack slot, size and the
//! instructions they occur at, and the return type. Required from every tool
//! (DecBench's native line-mapping contract, Noelo-Lab/decbench#129).
//!
//! FreshBench does not read types or variables out of the C text: a type is
//! scored only through this evidence, matched to DWARF by argument position
//! and stack slot. Every address is checked before it counts: it must be the
//! start of an instruction inside the function (`validate`). An output with no
//! line that maps to such an instruction is not scored at all.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LineMap {
    /// 1-based line of the output text
    pub line: u32,
    /// ELF virtual addresses of the instructions the line came from
    pub addresses: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    /// the type as the tool prints it (`unsigned int`, `char *`, `undefined4`)
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// position in the ABI argument list (0-based); None for a local
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_index: Option<u32>,
    /// stack slot, in the tool's own frame convention (calibrated when matched)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub line_mappings: Vec<LineMap>,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
}

fn addr(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => {
            let t = s.trim();
            match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                Some(h) => u64::from_str_radix(h, 16).ok(),
                None => t.parse().ok(),
            }
        }
        _ => None,
    }
}

fn addrs(v: Option<&serde_json::Value>) -> Vec<u64> {
    v.and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(addr).collect())
        .unwrap_or_default()
}

impl Evidence {
    /// Read the evidence fields of one tool record, whatever spelling the tool
    /// uses (`line_number`/`line`, `type`/`type_name`, numbers or `0x` strings).
    /// None when the record carries none of them.
    pub fn from_json(rec: &serde_json::Value) -> Option<Evidence> {
        let lm = rec.get("line_mappings").and_then(|v| v.as_array());
        let vars = rec.get("variables").and_then(|v| v.as_array());
        let ret = rec
            .get("return_type")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if lm.is_none() && vars.is_none() && ret.is_none() {
            return None;
        }
        let line_mappings = lm
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let line = u32::try_from(
                    m.get("line_number")
                        .or_else(|| m.get("line"))
                        .and_then(|l| l.as_u64())?,
                )
                .ok()?;
                Some(LineMap {
                    line,
                    addresses: addrs(m.get("addresses")),
                })
            })
            .collect();
        let variables = vars
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let s = |k: &str| v.get(k).and_then(|x| x.as_str());
                Some(Variable {
                    name: s("name").unwrap_or_default().to_string(),
                    type_name: s("type").or_else(|| s("type_name"))?.to_string(),
                    kind: s("kind").map(str::to_string),
                    arg_index: v
                        .get("arg_index")
                        .and_then(|x| x.as_u64())
                        .map(|x| x as u32),
                    stack_offset: v.get("stack_offset").and_then(|x| x.as_i64()),
                    size: v.get("size").and_then(|x| x.as_u64()).map(|x| x as u32),
                    addresses: addrs(v.get("addresses")),
                })
            })
            .collect();
        Some(Evidence {
            line_mappings,
            variables,
            return_type: ret,
        })
    }
}

/// Instruction starts of `[low, high)` in a binary: which addresses may count.
pub struct Starts {
    low: u64,
    high: u64,
    /// x86: the decoded instruction starts (sorted); else alignment is checked
    decoded: Option<Vec<u64>>,
    align: u64,
}

impl Starts {
    /// `code` holds the function's bytes, `code[0]` at `low`.
    pub fn new(arch: &str, low: u64, high: u64, code: &[u8]) -> Starts {
        let decoded = match arch {
            "x86_64" | "i686" => {
                use iced_x86::{Decoder, DecoderOptions};
                let bits = if arch == "x86_64" { 64 } else { 32 };
                let mut d = Decoder::with_ip(bits, code, low, DecoderOptions::NONE);
                let mut v = Vec::new();
                while d.can_decode() {
                    let i = d.decode();
                    v.push(i.ip());
                }
                Some(v)
            }
            _ => None,
        };
        Starts {
            low,
            high,
            decoded,
            // AArch64 and ARM: 4; Thumb-2 (ARMv7 here): 2
            align: if arch == "armv7" { 2 } else { 4 },
        }
    }

    pub fn contains(&self, a: u64) -> bool {
        if a < self.low || a >= self.high {
            return false;
        }
        match &self.decoded {
            Some(v) => v.binary_search(&a).is_ok(),
            None => (a - self.low) % self.align == 0,
        }
    }
}

/// The instruction starts of the function `[low, high)` of an unstripped
/// binary (`high` None: to the end of its section). None when `low` is in no
/// section with bytes.
pub fn starts_in(data: &[u8], arch: &str, low: u64, high: Option<u64>) -> Option<Starts> {
    use object::{Object, ObjectSection};
    let f = object::File::parse(data).ok()?;
    let s = f
        .sections()
        .find(|s| s.file_range().is_some() && s.address() <= low && low < s.address() + s.size())?;
    let end = s.address() + s.size();
    let high = high.unwrap_or(end).min(end).max(low);
    let bytes = s.data().ok()?;
    let a = (low - s.address()) as usize;
    let b = ((high - s.address()) as usize).min(bytes.len());
    Some(Starts::new(arch, low, high, &bytes[a.min(b)..b]))
}

/// What survived validation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checked {
    /// output lines with at least one valid instruction address
    pub mapped_lines: u32,
    /// non-blank lines of the output (the mapping coverage's denominator)
    #[serde(default)]
    pub code_lines: u32,
    pub valid_addresses: u32,
    pub dropped_addresses: u32,
    /// Distinct instruction addresses covered by valid output-line mappings.
    #[serde(default)]
    pub mapped_instructions: u32,
    /// Number of line records outside the output or on blank lines.
    #[serde(default)]
    pub invalid_lines: u32,
}

/// Validate output line numbers and instruction starts, merging repeated
/// records for the same line. Address counts describe input occurrences;
/// mapped_lines and mapped_instructions count distinct surviving items.
pub fn validate(ev: &mut Evidence, starts: &Starts, output: &str) -> Checked {
    use std::collections::{BTreeMap, BTreeSet};

    let lines: Vec<bool> = output.lines().map(|l| !l.trim().is_empty()).collect();
    let mut c = Checked {
        code_lines: lines.iter().filter(|&&line| line).count() as u32,
        ..Default::default()
    };
    let keep = |v: &mut Vec<u64>, c: &mut Checked| {
        let before = v.len();
        v.retain(|a| starts.contains(*a));
        c.valid_addresses += v.len() as u32;
        c.dropped_addresses += (before - v.len()) as u32;
        v.sort_unstable();
        v.dedup();
    };
    let mut by_line = BTreeMap::<u32, BTreeSet<u64>>::new();
    for mut m in std::mem::take(&mut ev.line_mappings) {
        if m.line == 0 || !lines.get(m.line as usize - 1).copied().unwrap_or(false) {
            c.invalid_lines += 1;
            c.dropped_addresses += m.addresses.len() as u32;
            continue;
        }
        keep(&mut m.addresses, &mut c);
        if !m.addresses.is_empty() {
            by_line.entry(m.line).or_default().extend(m.addresses);
        }
    }
    let mut instructions = BTreeSet::new();
    ev.line_mappings = by_line
        .into_iter()
        .map(|(line, addresses)| {
            instructions.extend(addresses.iter().copied());
            LineMap {
                line,
                addresses: addresses.into_iter().collect(),
            }
        })
        .collect();
    c.mapped_lines = ev.line_mappings.len() as u32;
    c.mapped_instructions = instructions.len() as u32;
    for v in &mut ev.variables {
        keep(&mut v.addresses, &mut c);
    }
    c
}

/// Test helper: an x86-64 ELF executable whose `.text` holds `code` at `addr`.
#[cfg(test)]
pub fn test_elf(addr: u64, code: &[u8]) -> Vec<u8> {
    use object::write::{Object, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };
    let mut o = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
    let text = o.section_id(object::write::StandardSection::Text);
    // pad so the code lands at `addr` inside the section
    let mut bytes = vec![0x90u8; addr as usize];
    bytes.extend_from_slice(code);
    o.set_section_data(text, bytes, 1);
    o.add_symbol(Symbol {
        name: b"f".to_vec(),
        value: addr,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let _ = SectionKind::Text;
    o.write().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_spelling() {
        let rec = serde_json::json!({
            "address": "0x10", "code": "int f(void){}",
            "line_mappings": [{"line_number": 3, "addresses": [4222405, "0x406dc6"]}, {"line": 4, "addresses": []}],
            "variables": [{"name": "param_1", "type": "uint", "arg_index": 0, "kind": "arg"},
                          {"name": "local_10", "type_name": "char *", "stack_offset": -16, "size": 8, "addresses": ["0x406dc5"]},
                          {"name": "no_type"}],
            "return_type": "int"
        });
        let e = Evidence::from_json(&rec).unwrap();
        assert_eq!(e.line_mappings[0].addresses, [4222405, 0x406dc6]);
        assert_eq!(e.line_mappings[1].line, 4);
        assert_eq!(e.variables.len(), 2);
        assert_eq!(e.variables[0].arg_index, Some(0));
        assert_eq!(e.variables[1].stack_offset, Some(-16));
        assert_eq!(e.return_type.as_deref(), Some("int"));
        assert!(Evidence::from_json(&serde_json::json!({"code": "x"})).is_none());
    }

    #[test]
    fn invalid_and_duplicate_lines_cannot_inflate_coverage() {
        let s = Starts::new("x86_64", 0x1000, 0x1002, &[0x90, 0xc3]);
        let mut e = Evidence {
            line_mappings: [(0, 0x1000), (1, 0x1000), (1, 0x1000), (1, 0x1001),
                (2, 0x1001), (4, 0x1000)]
                .into_iter()
                .map(|(line, address)| LineMap {
                    line,
                    addresses: vec![address],
                })
                .collect(),
            ..Default::default()
        };
        let c = validate(&mut e, &s, "return 1;\n\n}");
        assert_eq!((c.mapped_lines, c.code_lines, c.mapped_instructions), (1, 2, 2));
        assert_eq!(c.invalid_lines, 3);
        assert_eq!(e.line_mappings[0].addresses, [0x1000, 0x1001]);
    }

    #[test]
    fn only_instruction_starts_in_the_function_count() {
        // push rbp; mov rbp, rsp; mov eax, 1; pop rbp; ret
        let code = [0x55, 0x48, 0x89, 0xe5, 0xb8, 0x01, 0, 0, 0, 0x5d, 0xc3];
        let s = Starts::new("x86_64", 0x1000, 0x1000 + code.len() as u64, &code);
        assert!(
            s.contains(0x1000) && s.contains(0x1001) && s.contains(0x1004) && s.contains(0x100a)
        );
        // inside an instruction, before and after the function
        assert!(
            !s.contains(0x1002) && !s.contains(0x1005) && !s.contains(0xfff) && !s.contains(0x100b)
        );
        let mut e = Evidence {
            line_mappings: vec![
                LineMap {
                    line: 1,
                    addresses: vec![0x1004, 0x1005],
                },
                LineMap {
                    line: 2,
                    addresses: vec![0x2000],
                },
            ],
            variables: vec![Variable {
                addresses: vec![0x1001, 0x1003],
                ..Default::default()
            }],
            return_type: None,
        };
        let c = validate(&mut e, &s, "return 1;\nreturn 2;");
        assert_eq!(
            (c.mapped_lines, c.valid_addresses, c.dropped_addresses),
            (1, 2, 3)
        );
        assert_eq!(e.line_mappings[0].addresses, [0x1004]);
        // ARM: alignment only; Thumb-2 allows 2-byte steps
        let a64 = Starts::new("aarch64", 0x400, 0x410, &[]);
        assert!(a64.contains(0x404) && !a64.contains(0x406));
        let t2 = Starts::new("armv7", 0x400, 0x410, &[]);
        assert!(t2.contains(0x406) && !t2.contains(0x407));
        // from a binary: the section holding the function
        let elf = test_elf(0x10, &code);
        let s = starts_in(&elf, "x86_64", 0x10, None).unwrap();
        assert!(s.contains(0x10) && s.contains(0x14) && !s.contains(0x12));
    }
}
