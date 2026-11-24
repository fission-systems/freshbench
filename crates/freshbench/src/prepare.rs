//! Ground truth: which functions of a build are benchmark rows, from DWARF.
//!
//! A function is eligible in a build when all of these hold:
//! * it is a DWARF subprogram with a `low_pc` (name and decl_file read through
//!   `DW_AT_abstract_origin` / `DW_AT_specification`, so out-of-line bodies of
//!   partly inlined functions count) in a binary of the build;
//! * its compile unit is C and its `DW_AT_producer` shows the pinned compiler
//!   and optimization (the wrapper really ran);
//! * `low_pc` lies in an executable section that has bytes in the file (a
//!   `--gc-sections` tombstone or a debug-only file has no code);
//! * it is declared in its compile unit's primary source file, inside the
//!   project tree, outside submodules, third-party/test/example/benchmark/fuzz
//!   directories, and not in a well-known vendored single-file library;
//! * exactly one preprocessed text (system headers aside) exists for that
//!   source in the build, and that text defines the function exactly once
//!   (tree-sitter, `csyntax::definitions`);
//! * its name has one address in the binary;
//! * it does not appear in an excluded corpus (token fingerprint, see
//!   [`fingerprint`]).
//!
//! Selection: eligible functions in sha256("freshbench-v1/<project>/<source>/<name>")
//! order, first N per build. The key does not depend on architecture or
//! compiler, so the same functions are chosen across the matrix.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use gimli::{AttributeValue, EndianSlice, RunTimeEndian};
use object::{Object, ObjectSection, SectionKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::csyntax;
use crate::manifest::hex;

/// DW_LANG_C89, C, C99, C11, C17, and the two codes used for C23.
pub const C_LANGS: [u16; 7] = [0x1, 0x2, 0xC, 0x1D, 0x2C, 0x2D, 0x3E];

pub const EXCLUDED_DIRS: [&str; 39] = [
    "third_party",
    "third-party",
    "thirdparty",
    "3rd_party",
    "3rd-party",
    "3rdparty",
    "vendor",
    "vendored",
    "deps",
    "external",
    "extern",
    "contrib",
    "submodule",
    "submodules",
    "test",
    "tests",
    "testing",
    "testsuite",
    "unittest",
    "unittests",
    "t",
    "example",
    "examples",
    "demo",
    "demos",
    "sample",
    "samples",
    "bench",
    "benchmark",
    "benchmarks",
    "fuzz",
    "fuzzing",
    "fuzzer",
    "fuzzers",
    "Test",
    "Tests",
    "Examples",
    "Samples",
    "Benchmarks",
];

/// Well-known single-file libraries that projects copy in: not project code.
pub const VENDORED_BASENAMES: [&str; 66] = [
    "sqlite3.c",
    "shell.c",
    "cjson.c",
    "cjson_utils.c",
    "miniz.c",
    "lodepng.c",
    "ini.c",
    "linenoise.c",
    "jsmn.c",
    "parson.c",
    "sds.c",
    "ae.c",
    "anet.c",
    "zmalloc.c",
    "mongoose.c",
    "civetweb.c",
    "tinyexpr.c",
    "yxml.c",
    "argparse.c",
    "getopt.c",
    "getopt_long.c",
    "md5.c",
    "sha1.c",
    "sha256.c",
    "sha512.c",
    "crc32.c",
    "base64.c",
    "utf8proc.c",
    "xxhash.c",
    "lz4.c",
    "lz4hc.c",
    "zstd.c",
    "adler32.c",
    "inflate.c",
    "deflate.c",
    "trees.c",
    "inftrees.c",
    "inffast.c",
    "zutil.c",
    "lua.c",
    "lapi.c",
    "lvm.c",
    "stb_image.c",
    "stb_truetype.c",
    "nuklear.c",
    "cimgui.c",
    "microui.c",
    "log.c",
    "uthash.c",
    "list.c",
    "klib.c",
    "khash.c",
    "tinycthread.c",
    "pcg_basic.c",
    "mpc.c",
    "tweetnacl.c",
    "monocypher.c",
    "aes.c",
    "rijndael.c",
    "blake2b.c",
    "blake2s.c",
    "siphash.c",
    "murmurhash3.c",
    "fnmatch.c",
    "regex.c",
    "strlcpy.c",
];

/// Compiler/CRT-generated names that are never rows.
pub const SKIP_NAMES: [&str; 10] = [
    "_start",
    "_init",
    "_fini",
    "frame_dummy",
    "register_tm_clones",
    "deregister_tm_clones",
    "__do_global_dtors_aux",
    "__libc_csu_init",
    "__libc_csu_fini",
    "_dl_relocate_static_pie",
];

pub fn excluded_dir(rel_dir: &str) -> bool {
    rel_dir
        .split('/')
        .any(|c| EXCLUDED_DIRS.iter().any(|e| e.eq_ignore_ascii_case(c)))
}

/// The optimization flags the producer string must show for (compiler, config).
pub fn producer_ok(producer: &str, compiler: &str, config: &str) -> bool {
    if compiler == "gcc" && !producer.starts_with("GNU C") {
        return false;
    }
    if compiler == "clang" && !producer.contains("clang version") {
        return false;
    }
    let toks: Vec<&str> = producer.split_whitespace().collect();
    let is_opt = |t: &&&str| {
        t.starts_with("-O") && (t.len() == 2 || (t.len() == 3 && "0123sz".contains(&t[2..3])))
    };
    let want = if config == "O2-noinline" {
        "-O2".to_string()
    } else {
        format!("-{config}")
    };
    match toks.iter().rfind(is_opt) {
        Some(o) if *o == want => config != "O2-noinline" || toks.contains(&"-fno-inline"),
        _ => false,
    }
}

/// Normalize a path lexically (no filesystem access): drop `.`, resolve `..`.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// TU identity: sha256 of the preprocessed text outside system headers, line markers removed.
/// Two compiles of one file that differ only in system-header expansion are the same TU.
pub fn tu_identity(i_text: &str) -> String {
    let mut h = Sha256::new();
    let mut in_system = false;
    for line in i_text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            // # <line> "<file>" <flags...>; flag 3 = system header
            let mut parts = rest.split_whitespace();
            if parts
                .next()
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
            {
                let flags: Vec<&str> = rest
                    .rsplit('"')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .collect();
                in_system = flags.contains(&"3");
                continue;
            }
        }
        if !in_system {
            h.update(line.as_bytes());
            h.update(b"\n");
        }
    }
    hex(&h.finalize())
}

/// Token fingerprint of a function definition: sha256 of its tree-sitter leaf tokens
/// joined by single spaces (formatting and comments do not matter, renaming does).
pub fn fingerprint(def: tree_sitter::Node, src: &str) -> String {
    let mut h = Sha256::new();
    let mut stack = vec![def];
    let mut first = true;
    while let Some(n) = stack.pop() {
        if n.kind() == "comment" {
            continue;
        }
        if n.child_count() == 0 {
            if !first {
                h.update(b" ");
            }
            first = false;
            h.update(&src.as_bytes()[n.byte_range()]);
            continue;
        }
        let mut c = n.walk();
        let kids: Vec<_> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    hex(&h.finalize())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub name: String,
    pub source: String,
    pub address: u64,
    pub high_pc: Option<u64>,
    pub tu_sha256: String,
    pub fingerprint: String,
}

#[derive(Debug, Default, Serialize)]
pub struct BinaryResult {
    pub rows: BTreeMap<String, Candidate>,
    pub stats: BTreeMap<String, u64>,
}

struct DwarfFn {
    name: String,
    low: u64,
    high: Option<u64>,
    decl: Option<PathBuf>,
}

/// Every subprogram with a low_pc in C compile units: (cu primary source, producer, fns).
fn dwarf_functions(data: &[u8]) -> Result<Vec<(PathBuf, String, Vec<DwarfFn>)>> {
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
    let mut out = Vec::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let u = unit.unit_ref(&dwarf);
        let mut cursor = u.entries();
        let Some(top) = cursor.next_dfs()? else {
            continue;
        };
        let lang = match top.attr_value(gimli::DW_AT_language) {
            Some(AttributeValue::Language(l)) => l.0,
            _ => continue,
        };
        if !C_LANGS.contains(&lang) {
            continue;
        }
        let s = |v| attr_str(&u, v);
        let producer = s(top.attr_value(gimli::DW_AT_producer));
        let comp_dir = PathBuf::from(s(top.attr_value(gimli::DW_AT_comp_dir)));
        let cu_src = normalize(&comp_dir.join(s(top.attr_value(gimli::DW_AT_name))));
        // decl_file index -> normalized path
        let mut files: HashMap<u64, Option<PathBuf>> = HashMap::new();
        let file_path = |idx: u64| -> Option<PathBuf> {
            let lp = unit.line_program.as_ref()?;
            let hdr = lp.header();
            let fe = hdr.file(idx)?;
            let name = attr_str(&u, Some(fe.path_name()));
            let dir = attr_str(&u, fe.directory(hdr));
            Some(normalize(&comp_dir.join(dir).join(name)))
        };
        let mut fns = Vec::new();
        while let Some(die) = cursor.next_dfs()? {
            if die.tag() != gimli::DW_TAG_subprogram {
                continue;
            }
            let Some(lowv) = die.attr_value(gimli::DW_AT_low_pc) else {
                continue;
            };
            let Some(low) = u.attr_address(lowv)? else {
                continue;
            };
            let high = match die.attr_value(gimli::DW_AT_high_pc) {
                Some(AttributeValue::Udata(len)) => Some(low + len),
                Some(v) => u.attr_address(v)?,
                None => None,
            };
            // name and decl_file, following abstract_origin / specification
            let (mut name, mut decl) = (None, None);
            let mut cur = Some(die.clone());
            for _ in 0..4 {
                let Some(d) = cur.take() else { break };
                if name.is_none() {
                    if let Some(v) = d.attr_value(gimli::DW_AT_name) {
                        name = Some(attr_str(&u, Some(v))).filter(|n| !n.is_empty());
                    }
                }
                if decl.is_none() {
                    if let Some(AttributeValue::FileIndex(i)) = d.attr_value(gimli::DW_AT_decl_file)
                    {
                        decl = files.entry(i).or_insert_with(|| file_path(i)).clone();
                    }
                }
                if name.is_some() && decl.is_some() {
                    break;
                }
                let next = d
                    .attr_value(gimli::DW_AT_abstract_origin)
                    .or_else(|| d.attr_value(gimli::DW_AT_specification));
                if let Some(AttributeValue::UnitRef(off)) = next {
                    cur = unit.entry(off).ok();
                }
            }
            if let Some(name) = name {
                fns.push(DwarfFn {
                    name,
                    low,
                    high,
                    decl,
                });
            }
        }
        out.push((cu_src, producer, fns));
    }
    Ok(out)
}

fn attr_str<R: gimli::Reader>(u: &gimli::UnitRef<R>, v: Option<AttributeValue<R>>) -> String {
    match v.map(|v| u.attr_string(v)) {
        Some(Ok(r)) => r
            .to_string_lossy()
            .map(|c| c.into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Executable sections with bytes in the file: [start, end).
fn code_ranges(data: &[u8]) -> Result<Vec<(u64, u64)>> {
    let file = object::File::parse(data)?;
    Ok(file
        .sections()
        .filter(|s| {
            s.kind() == SectionKind::Text
                && s.size() > 0
                && s.file_range().is_some_and(|(_, n)| n > 0)
        })
        .map(|s| (s.address(), s.address() + s.size()))
        .collect())
}

pub struct BuildContext<'a> {
    pub compiler: &'a str,
    pub config: &'a str,
    pub work_dir: &'a str,
    pub submodules: &'a [String],
    /// realpath of each source -> {TU identity -> local .i path}
    pub tus: &'a BTreeMap<String, BTreeMap<String, PathBuf>>,
    pub excluded_fingerprints: &'a BTreeSet<String>,
}

/// Per-TU parse cache: TU identity -> function name -> (unique definition?, fingerprint).
pub type TuCache = HashMap<String, HashMap<String, Option<String>>>;

pub fn analyse_binary(data: &[u8], cx: &BuildContext, cache: &mut TuCache) -> Result<BinaryResult> {
    let mut r = BinaryResult::default();
    let mut bump = |k: &str| *r.stats.entry(k.to_string()).or_default() += 1;
    let ranges = code_ranges(data)?;
    let work = cx.work_dir.trim_end_matches('/');
    let mut found: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for (cu_src, producer, fns) in dwarf_functions(data)? {
        for f in fns {
            bump("dwarf_defined");
            if !ranges.iter().any(|(a, b)| *a <= f.low && f.low < *b) {
                bump("x_low_pc_not_in_code");
                continue;
            }
            if !producer_ok(&producer, cx.compiler, cx.config) {
                bump("x_producer");
                continue;
            }
            let Some(decl) = &f.decl else {
                bump("x_no_decl_file");
                continue;
            };
            if *decl != cu_src {
                bump("x_decl_not_primary");
                continue;
            }
            let src = cu_src.to_string_lossy().into_owned();
            let Some(rel) = src.strip_prefix(&format!("{work}/")) else {
                bump("x_outside_tree");
                continue;
            };
            if cx
                .submodules
                .iter()
                .any(|s| rel == s || rel.starts_with(&format!("{}/", s.trim_end_matches('/'))))
            {
                bump("x_submodule");
                continue;
            }
            let (dir, base) = rel.rsplit_once('/').unwrap_or(("", rel));
            if excluded_dir(dir) {
                bump("x_excluded_dir");
                continue;
            }
            if VENDORED_BASENAMES.contains(&base.to_ascii_lowercase().as_str()) {
                bump("x_vendored_basename");
                continue;
            }
            if SKIP_NAMES.contains(&f.name.as_str()) {
                bump("x_skip_name");
                continue;
            }
            let Some(texts) = cx.tus.get(&src) else {
                bump("x_no_preprocessed");
                continue;
            };
            if texts.len() != 1 {
                bump("x_ambiguous_tu");
                continue;
            }
            let (sha, ipath) = texts.iter().next().unwrap();
            let defs = cache.entry(sha.clone()).or_insert_with(|| {
                let text = std::fs::read(ipath)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let Ok(tree) = csyntax::parse(&text) else {
                    return HashMap::new();
                };
                csyntax::definitions(&tree, &text)
                    .into_iter()
                    .map(|(k, v)| (k, v.map(|n| fingerprint(n, &text))))
                    .collect()
            });
            let fp = match defs.get(&f.name) {
                Some(Some(fp)) => fp.clone(),
                Some(None) => {
                    bump("x_defined_twice_in_tu");
                    continue;
                }
                None => {
                    bump("x_not_in_tu");
                    continue;
                }
            };
            if cx.excluded_fingerprints.contains(&fp) {
                bump("x_excluded_corpus_overlap");
                continue;
            }
            found.entry(f.name.clone()).or_default().push(Candidate {
                name: f.name,
                source: rel.to_string(),
                address: f.low,
                high_pc: f.high,
                tu_sha256: sha.clone(),
                fingerprint: fp,
            });
        }
    }
    for (name, hits) in found {
        let addrs: BTreeSet<u64> = hits.iter().map(|h| h.address).collect();
        if addrs.len() != 1 {
            bump("x_name_not_unique_in_binary");
            continue;
        }
        r.rows.insert(name, hits.into_iter().next().unwrap());
    }
    Ok(r)
}

/// Selection key: sha256("freshbench-v1/<project>/<source>/<name>").
pub fn selection_key(project: &str, source: &str, name: &str) -> String {
    hex(&Sha256::digest(
        format!("freshbench-v1/{project}/{source}/{name}").as_bytes(),
    ))
}

#[derive(Deserialize)]
pub struct TuLogLine {
    pub src: String,
    pub i: String,
}

/// tu_log.jsonl + i/ -> realpath source -> {TU identity -> local .i}.
pub fn read_tus(build_dir: &Path) -> Result<BTreeMap<String, BTreeMap<String, PathBuf>>> {
    let mut out: BTreeMap<String, BTreeMap<String, PathBuf>> = BTreeMap::new();
    let log = build_dir.join("tu_log.jsonl");
    let text = std::fs::read_to_string(&log).with_context(|| format!("read {}", log.display()))?;
    let mut ids: HashMap<PathBuf, String> = HashMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let rec: TuLogLine = serde_json::from_str(line)?;
        let Some(name) = Path::new(&rec.i).file_name() else {
            continue;
        };
        let local = build_dir.join("i").join(name);
        if !local.exists() {
            continue;
        }
        let id = match ids.get(&local) {
            Some(id) => id.clone(),
            None => {
                let t = String::from_utf8_lossy(&std::fs::read(&local)?).into_owned();
                let id = tu_identity(&t);
                ids.insert(local.clone(), id.clone());
                id
            }
        };
        out.entry(rec.src).or_default().entry(id).or_insert(local);
    }
    Ok(out)
}

/// Check that stripping kept every code byte and removed symbols and debug info.
pub fn verify_strip(original: &[u8], stripped: &[u8]) -> Result<()> {
    let (a, b) = (
        object::File::parse(original)?,
        object::File::parse(stripped)?,
    );
    for s in a
        .sections()
        .filter(|s| s.kind() == SectionKind::Text && s.size() > 0)
    {
        let name = s.name()?;
        let t = b
            .section_by_name(name)
            .with_context(|| format!("stripped copy lost {name}"))?;
        if s.data()? != t.data()? || s.address() != t.address() {
            bail!("stripped copy changed {name}");
        }
    }
    for s in b.sections() {
        let n = s.name().unwrap_or("");
        if n.starts_with(".debug") || n.starts_with(".zdebug") || n == ".symtab" || n == ".strtab" {
            bail!("stripped copy still has {n}");
        }
    }
    if b.symbols().next().is_some() {
        bail!("stripped copy still has a symbol table");
    }
    Ok(())
}

#[derive(Deserialize)]
struct BuildJson {
    compiler: String,
    config: String,
    work_dir: String,
    binaries: Vec<BuildBin>,
}

#[derive(Deserialize)]
struct BuildBin {
    file: String,
}

#[derive(Debug, Default, Serialize)]
pub struct BuildResult {
    /// (source, name) -> (binary file, candidate); the first binary in file order owns a function
    pub eligible: BTreeMap<String, (String, Candidate)>,
    pub stats: BTreeMap<String, u64>,
}

/// Eligible functions of one build directory written by build_project.
pub fn analyse_build_dir(
    dir: &Path,
    project: &str,
    submodules: &[String],
    exclude: Option<&Path>,
) -> Result<BuildResult> {
    let meta: BuildJson = serde_json::from_slice(&std::fs::read(dir.join("build.json"))?)?;
    let tus = read_tus(dir)?;
    let excluded: BTreeSet<String> = match exclude {
        Some(p) => serde_json::from_slice(&std::fs::read(p)?)?,
        None => BTreeSet::new(),
    };
    let cx = BuildContext {
        compiler: &meta.compiler,
        config: &meta.config,
        work_dir: &meta.work_dir,
        submodules,
        tus: &tus,
        excluded_fingerprints: &excluded,
    };
    let mut cache = TuCache::new();
    let mut out = BuildResult::default();
    let mut files: Vec<_> = meta.binaries.iter().map(|b| b.file.clone()).collect();
    files.sort();
    for f in files {
        let data = std::fs::read(dir.join("bin").join(&f))?;
        let r = match analyse_binary(&data, &cx, &mut cache) {
            Ok(r) => r,
            Err(e) => {
                *out.stats
                    .entry(format!("binary_unreadable: {e}"))
                    .or_default() += 1;
                continue;
            }
        };
        for (k, v) in r.stats {
            *out.stats.entry(k).or_default() += v;
        }
        for (name, c) in r.rows {
            let key = format!("{}\u{0}{}", c.source, name);
            out.eligible.entry(key).or_insert((f.clone(), c));
        }
    }
    let _ = project;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producers() {
        let g = "GNU C17 16.2.0 -mtune=generic -march=x86-64 -g -O2 -fno-inline -fno-builtin";
        assert!(producer_ok(g, "gcc", "O2-noinline"));
        assert!(!producer_ok(g, "gcc", "O2") || g.contains("-fno-inline"));
        assert!(producer_ok("GNU C17 16.2.0 -g -O0", "gcc", "O0"));
        assert!(
            !producer_ok("GNU C17 16.2.0 -g -O0 -O2", "gcc", "O0"),
            "last -O wins"
        );
        assert!(producer_ok(
            "clang version 23.1.6 (x) -g -Os",
            "clang",
            "Os"
        ));
        assert!(!producer_ok("clang version 23.1.6 (x) -g -Os", "gcc", "Os"));
        assert!(!producer_ok("GNU C17 16.2.0 -g -O2", "gcc", "O2-noinline"));
    }

    #[test]
    fn dirs_and_paths() {
        assert!(excluded_dir("src/third_party/zlib"));
        assert!(excluded_dir("Tests"));
        assert!(!excluded_dir("src/testing_utils_not") || excluded_dir("testing"));
        assert!(!excluded_dir("src/core"));
        assert_eq!(
            normalize(Path::new("/w/a/./b/../c.c")),
            PathBuf::from("/w/a/c.c")
        );
    }

    #[test]
    fn tu_identity_ignores_system_headers_and_markers() {
        let a = "# 1 \"x.c\"\n# 1 \"/usr/include/stdio.h\" 1 3 4\nextern int printf();\n# 2 \"x.c\" 2\nint f(void){return 1;}\n";
        let b = "# 1 \"x.c\"\n# 1 \"/usr/include/stdio.h\" 1 3 4\nextern int printf(const char*, ...);\n# 5 \"x.c\" 2\nint f(void){return 1;}\n";
        let c = "# 1 \"x.c\"\nint f(void){return 2;}\n";
        assert_eq!(tu_identity(a), tu_identity(b));
        assert_ne!(tu_identity(a), tu_identity(c));
    }

    #[test]
    fn fingerprint_ignores_formatting_not_names() {
        let fp = |code: &str| {
            let t = csyntax::parse(code).unwrap();
            let d = csyntax::definitions(&t, code);
            fingerprint(d["f"].unwrap(), code)
        };
        assert_eq!(
            fp("int f(int a){return a+1;}"),
            fp("int f(int a)\n{\n  /* c */ return a + 1;\n}\n")
        );
        assert_ne!(
            fp("int f(int a){return a+1;}"),
            fp("int f(int b){return b+1;}")
        );
    }

    /// Linux only (needs an ELF toolchain): compile a two-function project with
    /// `cc -g -O0`, then check eligibility end to end. Skipped when `cc` is absent.
    #[cfg(target_os = "linux")]
    #[test]
    fn elf_end_to_end() {
        use std::process::Command;
        if Command::new("cc").arg("--version").output().is_err() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::create_dir_all(work.join("tests")).unwrap();
        std::fs::write(work.join("src/a.c"), "static int helper(int x) { return x * 3; }\nint api(int y) { return helper(y) + 1; }\nint main(void) { return api(2); }\n").unwrap();
        let src = work.join("src/a.c");
        let i = d.path().join("a.i");
        assert!(Command::new("cc")
            .args(["-g", "-O0", "-E"])
            .arg(&src)
            .arg("-o")
            .arg(&i)
            .status()
            .unwrap()
            .success());
        let bin = d.path().join("a.out");
        assert!(Command::new("cc")
            .args(["-g", "-O0", "-grecord-gcc-switches", "-fno-inline"])
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .current_dir(&work)
            .status()
            .unwrap()
            .success());
        let data = std::fs::read(&bin).unwrap();
        let text = std::fs::read_to_string(&i).unwrap();
        let mut tus = BTreeMap::new();
        tus.insert(
            src.to_string_lossy().into_owned(),
            [(tu_identity(&text), i.clone())].into_iter().collect(),
        );
        let producer_is_gcc =
            String::from_utf8_lossy(&Command::new("cc").arg("--version").output().unwrap().stdout)
                .contains("gcc");
        let cx = BuildContext {
            compiler: if producer_is_gcc { "gcc" } else { "clang" },
            config: "O0",
            work_dir: work.to_str().unwrap(),
            submodules: &[],
            tus: &tus,
            excluded_fingerprints: &BTreeSet::new(),
        };
        let r = analyse_binary(&data, &cx, &mut TuCache::new()).unwrap();
        if !producer_is_gcc {
            return; // clang records flags only with -grecord-command-line; the producer check is covered above
        }
        let names: Vec<_> = r.rows.keys().cloned().collect();
        assert_eq!(names, ["api", "helper", "main"], "stats: {:?}", r.stats);
        assert_eq!(r.rows["api"].source, "src/a.c");
        // the same functions are excluded by fingerprint
        let fps: BTreeSet<String> = r.rows.values().map(|c| c.fingerprint.clone()).collect();
        let cx2 = BuildContext {
            excluded_fingerprints: &fps,
            ..cx
        };
        let r2 = analyse_binary(&data, &cx2, &mut TuCache::new()).unwrap();
        assert!(r2.rows.is_empty());
        assert_eq!(r2.stats["x_excluded_corpus_overlap"], 3);
    }

    #[test]
    fn selection_key_is_stable() {
        assert_eq!(
            selection_key("p", "src/a.c", "f"),
            selection_key("p", "src/a.c", "f")
        );
        assert_ne!(
            selection_key("p", "src/a.c", "f"),
            selection_key("p", "src/b.c", "f")
        );
        assert_eq!(selection_key("p", "s", "f").len(), 64);
    }
}
