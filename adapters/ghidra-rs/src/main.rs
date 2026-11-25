//! FreshBench adapter for koute/ghidra-decompiler (`ghidra-rs`), a pure-Rust
//! port of Ghidra 12.2's C++ decompiler. See adapters/README.md.
//!
//! Decompile mode:
//!     run --binary <elf> --addresses <file> --elf-base 0x... --out <jsonl>
//! List mode:
//!     run --list-functions --binary <elf> --elf-base 0x... --out <jsonl>
//!
//! Evidence mirrors adapters/ghidra/FreshBenchDecompile.java. Ghidra's Java
//! side receives the C++ printer's markup (`EmitMarkup`) and turns it into
//! ClangTokens; here the same printer (`EmitPrettyPrint`, so the same line
//! breaking) drives a token sink instead of `EmitMarkup`, and receives the
//! same calls with the same op/varnode/symbol marks. `code` is rendered from
//! those token lines exactly as `DecompilerUtils.toLines` does, so line N
//! carries the instruction addresses of its tokens (`getMinAddress` per token
//! class). Variables are the function's local-scope symbols (Java's
//! `LocalSymbolMap`), the return type is the prototype's output type.
//!
//! Each function is analyzed once, by the same steps as `Program::decompile`
//! (`prepare` + `decompile_one`), so `code` is the text `decompile` prints.
//! The token sink also forwards every call to the library's plain-text
//! emitter; the token rendering must equal that text, or the function is
//! reported as an error rather than with mappings that do not belong to the
//! printed text. `GHIDRA_RS_VERIFY=1` additionally decompiles every function
//! with plain `Program::decompile` on a second copy and reports differences
//! on stderr (development check; equal on all 1,200 apulse functions).
//!
//! Environment: `GHIDRA_RS_FUNCTION_TIMEOUT` (seconds per function, default
//! 300, as the Ghidra adapter's `GHIDRA_FUNCTION_TIMEOUT`), `GHIDRA_RS_DEBUG`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghidra_decompiler::address::Address;
use ghidra_decompiler::architecture::Architecture;
use ghidra_decompiler::database::SymbolId;
use ghidra_decompiler::error::Result as GResult;
use ghidra_decompiler::funcdata::Funcdata;
use ghidra_decompiler::op::{OpId, PcodeOp};
use ghidra_decompiler::oplist::LinkedNode;
use ghidra_decompiler::prettyprint::{
    BlockMark, Emit, EmitBase, EmitNoMarkup, EmitPrettyPrint, FuncMark, OpMark, SymbolMark,
    SyntaxHighlight, TypeMark, VarnodeMark,
};
use ghidra_decompiler::printlanguage::PrintLanguage;
use ghidra_decompiler::program::{FunctionSource, Program, SymbolLoading};
use ghidra_decompiler::space::SpaceRef;
use ghidra_decompiler::types::TypeId;
use ghidra_decompiler::varnode::VarnodeId;

// ---------------------------------------------------------------- tokens

/// One markup token, as `ClangToken.buildToken` would classify it.
#[derive(Clone, Debug)]
enum Kind {
    /// ELEM_BREAK (ClangBreak)
    Break(i32),
    /// ELEM_SYNTAX (ClangSyntaxToken): no address
    Syntax,
    /// ELEM_VARIABLE (ClangVariableToken): address of `op`; `decl` is the
    /// enclosing ClangVariableDecl's symbol
    Var {
        vn: Option<VarnodeId>,
        op: Option<OpId>,
        decl: Option<SymbolId>,
    },
    /// ELEM_OP, ELEM_FUNCNAME, ELEM_VALUE (op, func name and case tokens):
    /// address of `op`
    WithOp(Option<OpId>),
    /// ELEM_TYPE, ELEM_FIELD, ELEM_BITFIELD: no address (Java does not
    /// override getMinAddress for them)
    NoAddr,
    /// ELEM_COMMENT (ClangCommentToken): its source address
    Comment(Option<u64>),
    /// ELEM_LABEL (ClangLabelToken): the block address
    Label(Option<u64>),
}

#[derive(Clone, Debug)]
struct Tok {
    text: String,
    hl: SyntaxHighlight,
    kind: Kind,
}

type Sink = Arc<Mutex<Vec<Tok>>>;

/// The printer's low-level emitter: receives what `EmitMarkup` would encode
/// (Ghidra's Java side builds its ClangTokens from that) and records it as
/// tokens. Every call is also forwarded to the library's plain-text emitter
/// (`EmitNoMarkup`, what `Program::decompile` prints through), whose output is
/// the reference the token rendering is checked against.
struct TokenEmit {
    base: EmitBase,
    sink: Sink,
    decl: Vec<Option<SymbolId>>,
    plain: EmitNoMarkup,
}

impl TokenEmit {
    fn push(&mut self, text: &str, hl: SyntaxHighlight, kind: Kind) {
        self.sink.lock().expect("token sink").push(Tok {
            text: text.to_string(),
            hl,
            kind,
        });
    }
}

fn addr_of(spc: Option<&SpaceRef>, off: u64) -> Option<u64> {
    spc.map(|_| off)
}

impl Emit for TokenEmit {
    fn base(&self) -> &EmitBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut EmitBase {
        &mut self.base
    }
    fn as_dyn(&mut self) -> &mut dyn Emit {
        self
    }
    fn begin_document(&mut self) -> GResult<i32> {
        self.plain.begin_document()
    }
    fn end_document(&mut self, id: i32) -> GResult<()> {
        self.plain.end_document(id)
    }
    fn begin_function(&mut self, fd: Option<&FuncMark>) -> GResult<i32> {
        self.plain.begin_function(fd)
    }
    fn end_function(&mut self, id: i32) -> GResult<()> {
        self.plain.end_function(id)
    }
    fn begin_block(&mut self, bl: Option<BlockMark>) -> GResult<i32> {
        self.plain.begin_block(bl)
    }
    fn end_block(&mut self, id: i32) -> GResult<()> {
        self.plain.end_block(id)
    }
    fn tag_line(&mut self) -> GResult<()> {
        self.emit_pending()?;
        let indent = self.base.indentlevel;
        self.push("", SyntaxHighlight::NoColor, Kind::Break(indent));
        self.plain.tag_line_indent(indent)
    }
    fn tag_line_indent(&mut self, indent: i32) -> GResult<()> {
        self.emit_pending()?;
        self.push("", SyntaxHighlight::NoColor, Kind::Break(indent));
        self.plain.tag_line_indent(indent)
    }
    fn begin_return_type(&mut self, vn: Option<VarnodeMark>) -> GResult<i32> {
        self.plain.begin_return_type(vn)
    }
    fn end_return_type(&mut self, id: i32) -> GResult<()> {
        self.plain.end_return_type(id)
    }
    fn begin_var_decl(&mut self, sym: Option<SymbolMark>) -> GResult<i32> {
        self.decl.push(sym.map(|m| m.id));
        self.plain.begin_var_decl(sym)
    }
    fn end_var_decl(&mut self, id: i32) -> GResult<()> {
        self.decl.pop();
        self.plain.end_var_decl(id)
    }
    fn begin_statement(&mut self, op: Option<OpMark>) -> GResult<i32> {
        self.plain.begin_statement(op)
    }
    fn end_statement(&mut self, id: i32) -> GResult<()> {
        self.plain.end_statement(id)
    }
    fn begin_func_proto(&mut self) -> GResult<i32> {
        self.plain.begin_func_proto()
    }
    fn end_func_proto(&mut self, id: i32) -> GResult<()> {
        self.plain.end_func_proto(id)
    }
    fn tag_variable(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        vn: Option<VarnodeMark>,
        op: Option<OpMark>,
    ) -> GResult<()> {
        let decl = self.decl.last().copied().flatten();
        self.push(
            name,
            hl,
            Kind::Var {
                vn: vn.map(|m| m.id),
                op: op.map(|m| m.id),
                decl,
            },
        );
        self.plain.tag_variable(name, hl, vn, op)
    }
    fn tag_op(&mut self, name: &str, hl: SyntaxHighlight, op: Option<OpMark>) -> GResult<()> {
        self.push(name, hl, Kind::WithOp(op.map(|m| m.id)));
        self.plain.tag_op(name, hl, op)
    }
    fn tag_func_name(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        fd: Option<&FuncMark>,
        op: Option<OpMark>,
    ) -> GResult<()> {
        self.push(name, hl, Kind::WithOp(op.map(|m| m.id)));
        self.plain.tag_func_name(name, hl, fd, op)
    }
    fn tag_type(&mut self, name: &str, hl: SyntaxHighlight, ct: Option<&TypeMark>) -> GResult<()> {
        self.push(name, hl, Kind::NoAddr);
        self.plain.tag_type(name, hl, ct)
    }
    fn tag_field(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        ct: Option<&TypeMark>,
        off: i32,
        op: Option<OpMark>,
    ) -> GResult<()> {
        self.push(name, hl, Kind::NoAddr);
        self.plain.tag_field(name, hl, ct, off, op)
    }
    fn tag_bit_field(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        ct: Option<&TypeMark>,
        id: i32,
        op: Option<OpMark>,
    ) -> GResult<()> {
        self.push(name, hl, Kind::NoAddr);
        self.plain.tag_bit_field(name, hl, ct, id, op)
    }
    fn tag_comment(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        spc: Option<&SpaceRef>,
        off: u64,
    ) -> GResult<()> {
        self.push(name, hl, Kind::Comment(addr_of(spc, off)));
        self.plain.tag_comment(name, hl, spc, off)
    }
    fn tag_label(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        spc: Option<&SpaceRef>,
        off: u64,
    ) -> GResult<()> {
        self.push(name, hl, Kind::Label(addr_of(spc, off)));
        self.plain.tag_label(name, hl, spc, off)
    }
    fn tag_case_label(
        &mut self,
        name: &str,
        hl: SyntaxHighlight,
        op: Option<OpMark>,
        value: u64,
    ) -> GResult<()> {
        self.push(name, hl, Kind::WithOp(op.map(|m| m.id)));
        self.plain.tag_case_label(name, hl, op, value)
    }
    fn print(&mut self, data: &str, hl: SyntaxHighlight) -> GResult<()> {
        self.push(data, hl, Kind::Syntax);
        self.plain.print(data, hl)
    }
    fn open_paren(&mut self, paren: &str, id: i32) -> GResult<i32> {
        self.push(paren, SyntaxHighlight::NoColor, Kind::Syntax);
        self.base.parenlevel += 1;
        self.plain.open_paren(paren, id)
    }
    fn close_paren(&mut self, paren: &str, id: i32) -> GResult<()> {
        self.push(paren, SyntaxHighlight::NoColor, Kind::Syntax);
        self.base.parenlevel -= 1;
        self.plain.close_paren(paren, id)
    }
    fn flush(&mut self) -> GResult<()> {
        self.plain.flush()
    }
    fn set_output_stream(&mut self, text: Vec<u8>) {
        self.plain.set_output_stream(text);
    }
    fn get_output_stream(&mut self) -> &mut Vec<u8> {
        self.plain.get_output_stream()
    }
    fn emits_markup(&self) -> bool {
        true
    }
}

fn printer(arch: &mut Architecture) -> Result<&mut Box<dyn PrintLanguage>, String> {
    let index = arch.print;
    arch.printlist
        .get_mut(index)
        .and_then(|p| p.as_mut())
        .ok_or_else(|| "no print language".to_string())
}

/// Give the current print language a new pretty printer whose
/// low-level emitter is the token sink, with the printer's line width,
/// indent and comment settings, in place of the printer's own.
fn install_sink(arch: &mut Architecture) -> Result<Sink, String> {
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let printer = printer(arch)?;
    let max_line = printer.base().emit.get_max_line_size();
    let indent = printer.base().emit.get_indent_increment();
    let style = match printer.base().commentstart.as_str() {
        "// " => "cplusplus",
        _ => "c",
    };
    let mut pretty = EmitPrettyPrint::new();
    pretty.lowlevel = Box::new(TokenEmit {
        base: EmitBase::new(),
        sink: sink.clone(),
        decl: Vec::new(),
        plain: EmitNoMarkup::new(),
    });
    printer.base_mut().emit = Box::new(pretty);
    printer
        .set_max_line_size(max_line)
        .map_err(|e| e.to_string())?;
    printer.set_indent_increment(indent);
    printer
        .set_comment_style(style)
        .map_err(|e| e.to_string())?;
    Ok(sink)
}

// ---------------------------------------------------------------- rendering

struct Rendered {
    code: String,
    /// (1-based line, instruction addresses)
    lines: Vec<(usize, BTreeSet<u64>)>,
    /// symbol -> addresses of the variable tokens that use it
    uses: BTreeMap<SymbolId, BTreeSet<u64>>,
}

/// One output line: indent and (text, address, variable symbol) per token.
struct Line {
    indent: i32,
    toks: Vec<(String, Option<u64>, Option<SymbolId>)>,
}

/// `DecompilerUtils.toLines` + the Java adapter's render loop.
fn render(toks: &[Tok], function: &mut Funcdata, arch: &Architecture) -> Rendered {
    let op_addr = |function: &Funcdata, op: Option<OpId>| -> Option<u64> {
        op.map(|id| function.op(id).get_addr().get_offset())
    };
    let mut lines: Vec<Line> = Vec::new();
    let mut i = 0;
    let mut current = match toks.first() {
        Some(Tok {
            kind: Kind::Break(indent),
            ..
        }) => {
            i = 1;
            Line {
                indent: *indent,
                toks: Vec::new(),
            }
        }
        _ => Line {
            indent: 0,
            toks: Vec::new(),
        },
    };
    while i < toks.len() {
        let t = &toks[i];
        match &t.kind {
            Kind::Break(indent) => {
                lines.push(std::mem::replace(
                    &mut current,
                    Line {
                        indent: *indent,
                        toks: Vec::new(),
                    },
                ));
                i += 1;
            }
            Kind::Comment(addr) => {
                // consumeCommentTokens: a run of comment tokens of one syntax
                // type and blank syntax tokens becomes one token at the first
                // comment's address
                let mut text = t.text.clone();
                i += 1;
                while i < toks.len() {
                    let u = &toks[i];
                    match &u.kind {
                        Kind::Comment(_) if u.hl == t.hl => text.push_str(&u.text),
                        Kind::Syntax if u.text.trim().is_empty() => text.push_str(&u.text),
                        _ => break,
                    }
                    i += 1;
                }
                current.toks.push((text, *addr, None));
            }
            kind => {
                let (addr, sym) = match kind {
                    Kind::Var { vn, op, decl } => {
                        let a = op_addr(function, *op);
                        let sym = match vn {
                            Some(vn) => match function.vn(*vn).get_high_option() {
                                Some(h) => function.high_get_symbol(h, arch),
                                None => None,
                            },
                            None => *decl,
                        };
                        (a, sym)
                    }
                    Kind::WithOp(op) => (op_addr(function, *op), None),
                    Kind::Label(a) => (*a, None),
                    _ => (None, None),
                };
                current.toks.push((t.text.clone(), addr, sym));
                i += 1;
            }
        }
    }
    lines.push(current);

    let mut code = String::new();
    let mut maps = Vec::new();
    let mut uses: BTreeMap<SymbolId, BTreeSet<u64>> = BTreeMap::new();
    for (n, line) in lines.iter().enumerate() {
        for _ in 0..line.indent.max(0) {
            code.push(' ');
        }
        let mut at = BTreeSet::new();
        for (text, addr, sym) in &line.toks {
            code.push_str(text);
            if let Some(a) = addr {
                at.insert(*a);
                if let Some(s) = sym {
                    uses.entry(*s).or_default().insert(*a);
                }
            }
        }
        code.push('\n');
        if !at.is_empty() {
            maps.push((n + 1, at));
        }
    }
    Rendered {
        code,
        lines: maps,
        uses,
    }
}

fn type_name(arch: &Architecture, tid: TypeId) -> String {
    match arch.types.as_deref() {
        Some(types) => {
            let mut s = String::new();
            types.get(tid).print_raw(&mut s, types);
            s
        }
        None => String::new(),
    }
}

fn sign_extend(off: u64, bytes: u32) -> i64 {
    if bytes == 0 || bytes >= 8 {
        return off as i64;
    }
    let shift = 64 - 8 * bytes;
    ((off << shift) as i64) >> shift
}

/// The fields after "address" of one decompiled function (JSON).
fn evidence_json(r: &Rendered, function: &mut Funcdata, arch: &Architecture) -> String {
    let mut lm = String::from("[");
    for (n, at) in &r.lines {
        if lm.len() > 1 {
            lm.push(',');
        }
        lm.push_str(&format!(
            "{{\"line_number\":{n},\"addresses\":{}}}",
            addr_list(at)
        ));
    }
    lm.push(']');

    let mut vars = String::from("[");
    let stack = arch.manager.get_stack_space();
    if let (Some(scope), Some(db)) = (function.get_scope_local(), arch.symboltab.as_deref()) {
        let syms: Vec<SymbolId> = db.scope(scope).nametree.values().copied().collect();
        for sym in syms {
            let s = db.symbol(sym);
            let tname = s.get_type().map(|t| type_name(arch, t)).unwrap_or_default();
            let tsize = s
                .get_type()
                .and_then(|t| arch.types.as_deref().map(|ty| ty.get(t).get_size()))
                .unwrap_or(0);
            let is_param = s.get_category() == 0;
            // HighSymbol.getStorage()/getSize(): the first entry of the symbol
            let (stack_off, size) = match s.mapentry.first().map(|e| db.entry(*e)) {
                Some(e) if e.is_dynamic() => (None, tsize),
                Some(e) => {
                    let a = e.get_addr();
                    let on_stack = match (&stack, a.get_space()) {
                        (Some(st), Some(sp)) => st.get_index() == sp.get_index(),
                        _ => false,
                    };
                    let off = if on_stack {
                        let bytes = a.get_space().map(|sp| sp.get_addr_size()).unwrap_or(8);
                        Some(sign_extend(a.get_offset(), bytes))
                    } else {
                        None
                    };
                    (off, e.get_size())
                }
                None => (None, tsize),
            };
            let kind = if is_param {
                "arg"
            } else if stack_off.is_some() {
                "stack"
            } else {
                "register"
            };
            if vars.len() > 1 {
                vars.push(',');
            }
            vars.push_str(&format!(
                "{{\"name\":{},\"type\":{},\"kind\":\"{kind}\",\"arg_index\":{},\"stack_offset\":{},\"size\":{size},\"addresses\":{}}}",
                json(s.get_name()),
                json(&tname),
                if is_param {
                    s.get_category_index().to_string()
                } else {
                    "null".into()
                },
                stack_off.map_or("null".into(), |o| o.to_string()),
                addr_list(r.uses.get(&sym).unwrap_or(&BTreeSet::new())),
            ));
        }
    }
    vars.push(']');

    let ret = function
        .get_func_proto()
        .get_output_ref()
        .map(|p| type_name(arch, p.get_type(arch)));
    format!(
        ",\"code\":{},\"line_mappings\":{lm},\"variables\":{vars},\"return_type\":{}",
        json(&r.code),
        ret.as_deref().map_or("null".into(), json)
    )
}

fn addr_list(s: &BTreeSet<u64>) -> String {
    let v: Vec<String> = s.iter().map(|a| a.to_string()).collect();
    format!("[{}]", v.join(","))
}

fn json(s: &str) -> String {
    let mut b = String::with_capacity(s.len() + 2);
    b.push('"');
    for c in s.chars() {
        match c {
            '"' => b.push_str("\\\""),
            '\\' => b.push_str("\\\\"),
            '\n' => b.push_str("\\n"),
            '\r' => b.push_str("\\r"),
            '\t' => b.push_str("\\t"),
            c if (c as u32) < 0x20 => b.push_str(&format!("\\u{:04x}", c as u32)),
            c => b.push(c),
        }
    }
    b.push('"');
    b
}

// ---------------------------------------------------------------- decompile

struct Session {
    program: Program,
    sink: Sink,
}

fn open(binary: &Path) -> Result<Program, String> {
    std::panic::catch_unwind(|| Program::open(binary, None))
        .map_err(|_| "panic while loading the binary".to_string())?
        .map_err(|e| e.to_string())
}

/// Load the binary, install the token sink and set the instruction set of
/// every requested start (ARM: TMode, as the Ghidra adapter does).
fn session(binary: &Path, starts: &[(u64, bool)], tokens: bool) -> Result<Session, String> {
    let mut program = open(binary)?;
    let arch = program.architecture();
    let sink = match tokens {
        true => install_sink(arch)?,
        false => Arc::new(Mutex::new(Vec::new())),
    };
    let space = arch
        .manager
        .get_default_code_space()
        .ok_or("no default code space")?;
    let mut context_changed = false;
    if let Some(ctx) = arch.context.clone() {
        let mut ctx = ctx.lock().map_err(|_| "context database lock")?;
        if ctx.get_variable("TMode").is_ok() && !starts.is_empty() {
            // each start's mode holds up to the next start (ascending order;
            // set_variable sets up to the next change point)
            let mut sorted: Vec<(u64, bool)> = starts.to_vec();
            sorted.sort();
            sorted.dedup_by_key(|s| s.0);
            for (va, thumb) in sorted {
                ctx.set_variable("TMode", &Address::new(space.clone(), va), u32::from(thumb))
                    .map_err(|e| e.to_string())?;
            }
            context_changed = true;
        }
    }
    if context_changed {
        // Loading already decoded instructions (the import stubs), and the
        // SLEIGH decoder caches the context region of its last lookup and
        // parsed instructions by address only. Give it the caches of a
        // freshly loaded copy that has decoded nothing, so the new context
        // is read.
        let fresh =
            std::panic::catch_unwind(|| Program::open_with(binary, None, SymbolLoading::Loader))
                .map_err(|_| "panic while loading the binary".to_string())?
                .map_err(|e| e.to_string())?;
        let mut fresh = fresh;
        let state = fresh
            .architecture()
            .translator()
            .map_err(|e| e.to_string())?
            .as_sleigh()
            .ok_or("translator is not SLEIGH")?
            .save_state();
        program
            .architecture()
            .translator()
            .map_err(|e| e.to_string())?
            .as_sleigh()
            .ok_or("translator is not SLEIGH")?
            .restore_state(state);
    }
    // every requested start is a function before any is decompiled, so one
    // function's output does not depend on which others were decompiled
    // before it (`set_function_name` with an empty name creates the function
    // when none starts there, as `decompile` does, and keeps the loaded name)
    for (va, _) in starts {
        if let Err(e) = program.set_function_name(*va, "") {
            eprintln!("ghidra-rs: cannot create a function at {va:#x}: {e}");
        }
    }
    Ok(Session { program, sink })
}

/// An action group with no actions: making it current leaves no current
/// action, and keeps it that way when the jump-table recovery inside the
/// flow restores the current action by name.
const NO_ANALYSIS: &str = "ghidra-rs-no-analysis";

/// What `Program::decompile` does before it analyzes: create the function
/// when none starts at `va` and add the string-literal symbols of the
/// function's constants. Both steps are private to the library, so they run
/// through `decompile` itself with no current action: it stops with "No
/// action set" right where the analysis would start (the analysis then runs
/// once, in `decompile_one`). The action database is put back as it was.
fn prepare(program: &mut Program, va: u64) -> Result<(), String> {
    let acts = &mut program.architecture().allacts;
    let name = acts.currentactname.clone();
    let current = acts.currentact.clone();
    let action = current.as_ref().and_then(|n| acts.actionmap.remove(n));
    acts.set_group(NO_ANALYSIS, &[]);
    let r = match acts.set_current(NO_ANALYSIS) {
        Ok(_) => program.decompile(va).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let acts = &mut program.architecture().allacts;
    acts.groupmap.remove(NO_ANALYSIS);
    acts.actionmap.remove(NO_ANALYSIS);
    if let (Some(n), Some(a)) = (current.clone(), action) {
        acts.actionmap.insert(n, a);
    }
    acts.currentact = current;
    acts.currentactname = name;
    match r {
        Err(e) if e.contains("No action set") => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err("internal: decompile analyzed without a current action".into()),
    }
}

/// Decompile one function: JSON fields after "address", or the error text.
fn decompile_one(s: &mut Session, va: u64) -> Result<(String, String), String> {
    prepare(&mut s.program, va)?;
    s.sink.lock().map_err(|_| "token sink")?.clear();
    let sink = s.sink.clone();
    let arch = s.program.architecture();
    let space = arch
        .manager
        .get_default_code_space()
        .ok_or("no default code space")?;
    let addr = Address::new(space, va);
    let db = arch.symboltab.as_deref().ok_or("no symbol table")?;
    let global = db.get_global_scope().ok_or("no global scope")?;
    let sym = db
        .scope_query_function(global, &addr)
        .ok_or_else(|| format!("no function at {va:#x}"))?;
    // the rest of `Program::decompile`, printed through the token sink
    let fields = arch
        .with_function(sym, |function, architecture| {
            if function.has_no_code() {
                return Err(ghidra_decompiler::error::Error::Lowlevel(format!(
                    "No code for {}",
                    function.get_name()
                )));
            }
            if function.is_proc_started() {
                architecture.clear_analysis(function);
            }
            let analysis = architecture.with_current_action(|action, architecture| {
                action.reset(function, architecture);
                action.perform(function, architecture)
            });
            let printed = analysis.and_then(|_| {
                architecture
                    .with_printer(Some(&mut *function), |printer, context| {
                        printer.doc_function(context)
                    })
                    .map(|((), text)| text)
            });
            let out = printed.map(|text| {
                let toks = std::mem::take(&mut *sink.lock().expect("token sink"));
                let first_break = matches!(
                    toks.first(),
                    Some(Tok {
                        kind: Kind::Break(_),
                        ..
                    })
                );
                let r = render(&toks, function, architecture);
                (
                    text,
                    r.code.clone(),
                    first_break,
                    evidence_json(&r, function, architecture),
                )
            });
            architecture.clear_analysis(function);
            out
        })
        .map_err(|e| e.to_string())?;
    let (reference, code, first_break, json_fields) = fields;
    // toLines drops a leading break (the plain text starts with its newline)
    // and always keeps the last line
    if code != expected_code(&reference, first_break) {
        if std::env::var_os("GHIDRA_RS_DEBUG").is_some() {
            eprintln!("--- plain {va:#x}\n{reference:?}\n--- tokens\n{code:?}");
        }
        return Err("token rendering differs from the plain-text printer".into());
    }
    Ok((code, json_fields))
}

fn expected_code(plain: &str, first_break: bool) -> String {
    match first_break {
        true => format!("{}\n", plain.strip_prefix('\n').unwrap_or(plain)),
        false => format!("{plain}\n"),
    }
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown panic".into())
}

/// Decompile `todo` (index into `reqs`) on its own thread; results go to `tx`.
fn spawn_worker(
    binary: PathBuf,
    starts: Vec<(u64, bool)>,
    todo: Vec<(usize, u64)>,
    tx: mpsc::Sender<(usize, Result<String, String>)>,
) {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let mut s = match session(&binary, &starts, true) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("ghidra-rs: cannot load {}: {e}", binary.display());
                    std::process::exit(1);
                }
            };
            // GHIDRA_RS_VERIFY=1: decompile the same functions in the same
            // order with plain `Program::decompile` on a second copy and
            // compare (stderr only; a development check)
            let mut verify = match std::env::var_os("GHIDRA_RS_VERIFY") {
                Some(_) => session(&binary, &starts, false).ok(),
                None => None,
            };
            let (mut same, mut diff) = (0, 0);
            for (i, va) in todo {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    decompile_one(&mut s, va)
                }))
                .unwrap_or_else(|p| Err(format!("panic: {}", panic_text(p))));
                if let Some(v) = verify.as_mut() {
                    let plain = v.program.decompile(va);
                    let ok = match (&r, &plain) {
                        (Ok((code, _)), Ok(t)) => {
                            *code == expected_code(t, true) || *code == expected_code(t, false)
                        }
                        (Err(_), Err(_)) => true,
                        _ => false,
                    };
                    if ok {
                        same += 1;
                    } else {
                        diff += 1;
                        eprintln!("ghidra-rs verify: {va:#x} differs from Program::decompile");
                    }
                }
                if tx.send((i, r.map(|(_, fields)| fields))).is_err() {
                    return;
                }
            }
            if verify.is_some() {
                eprintln!("ghidra-rs verify: {same} same, {diff} different");
            }
        })
        .expect("spawn decompiler thread");
}

fn decompile_mode(binary: &Path, addresses: &Path, out: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(addresses).map_err(|e| e.to_string())?;
    // (address as requested, ELF address of the first instruction, Thumb)
    let mut reqs: Vec<(String, u64, bool)> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let v = parse_addr(l).ok_or_else(|| format!("bad address {l}"))?;
        reqs.push((l.to_string(), v & !1, v & 1 == 1));
    }
    let timeout = Duration::from_secs(
        std::env::var("GHIDRA_RS_FUNCTION_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300),
    );
    // without a TMode register an odd address is not a Thumb start: keep it
    // as requested (the library then reports what it finds there)
    let mut probe = session(binary, &[], false)?;
    let has_tmode = probe
        .program
        .architecture()
        .context
        .as_ref()
        .is_some_and(|c| c.lock().is_ok_and(|c| c.get_variable("TMode").is_ok()));
    drop(probe);
    if !has_tmode {
        for r in &mut reqs {
            if r.2 {
                r.1 |= 1;
                r.2 = false;
            }
        }
    }
    let starts: Vec<(u64, bool)> = reqs.iter().map(|r| (r.1, r.2)).collect();
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
    let mut next = 0;
    while next < reqs.len() {
        let (tx, rx) = mpsc::channel();
        let todo: Vec<(usize, u64)> = (next..reqs.len()).map(|i| (i, reqs[i].1)).collect();
        spawn_worker(binary.to_path_buf(), starts.clone(), todo, tx);
        while next < reqs.len() {
            let r = match rx.recv_timeout(timeout) {
                Ok((i, r)) => {
                    debug_assert_eq!(i, next);
                    r
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err(format!("timeout after {} s", timeout.as_secs()))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Err("decompiler thread exited".into()),
            };
            let timed_out = matches!(&r, Err(e) if e.starts_with("timeout after"));
            let line = match r {
                Ok(fields) => format!(
                    "{{\"address\":{}{fields},\"error\":null}}",
                    json(&reqs[next].0)
                ),
                Err(e) => format!(
                    "{{\"address\":{},\"code\":null,\"error\":{}}}",
                    json(&reqs[next].0),
                    json(&e)
                ),
            };
            writeln!(f, "{line}").map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
            next += 1;
            if timed_out {
                // the stuck thread keeps the old program; start over with a
                // fresh one for the rest (it dies with the process)
                break;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- list mode

/// One past the highest byte the decompiler's own flow reached from `va`
/// (`FlowInfo` maxaddr = lowest flowed address + `Funcdata::get_size`).
fn body_end(program: &mut Program, va: u64) -> Option<u64> {
    let arch = program.architecture();
    let space = arch.manager.get_default_code_space()?;
    let addr = Address::new(space.clone(), va);
    let db = arch.symboltab.as_deref()?;
    let sym = db.scope_query_function(db.get_global_scope()?, &addr)?;
    arch.with_function(sym, |function, architecture| {
        if function.has_no_code() {
            return Ok(None);
        }
        architecture.clear_analysis(function);
        let start = Address::from_parts(Some(space.clone()), 0);
        let end = Address::from_parts(Some(space.clone()), space.get_highest());
        if function.follow_flow(&start, &end, architecture).is_err() {
            architecture.clear_analysis(function);
            return Ok(None);
        }
        let mut lo = function.get_address().get_offset();
        let mut pos = function.begin_op_alive();
        while let Some(op) = pos {
            let o = function.op(op);
            lo = lo.min(o.get_addr().get_offset());
            pos = o.links(PcodeOp::INSERT_LIST).next;
        }
        let size = function.get_size();
        architecture.clear_analysis(function);
        Ok((size > 0).then(|| lo + size as u64))
    })
    .ok()
    .flatten()
}

fn list_mode(binary: &Path, out: &Path) -> Result<(), String> {
    let mut program = open(binary)?;
    program.discover_functions().map_err(|e| e.to_string())?;
    let functions = program.functions().map_err(|e| e.to_string())?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
    for func in functions {
        if std::env::var_os("GHIDRA_RS_DEBUG").is_some() {
            eprintln!("{:#x} {} {:?}", func.address, func.name, func.source);
        }
        // import stubs are the library's own "not a function of this binary"
        if func.source == FunctionSource::ImportStub {
            continue;
        }
        let end = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            body_end(&mut program, func.address)
        }))
        .unwrap_or(None);
        let end = end.map_or("null".to_string(), |e| format!("\"{e:#x}\""));
        writeln!(f, "{{\"start\":\"{:#x}\",\"end\":{end}}}", func.address)
            .map_err(|e| e.to_string())?;
    }
    f.flush().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- main

fn parse_addr(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

fn main() {
    // panics are reported per function in `error`; keep stderr to one line each
    std::panic::set_hook(Box::new(|info| {
        eprintln!("ghidra-rs: panic: {info}");
    }));
    let mut args = std::env::args().skip(1);
    let (mut binary, mut addresses, mut base, mut out, mut list) = (None, None, None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--binary" => binary = args.next().map(PathBuf::from),
            "--addresses" => addresses = args.next().map(PathBuf::from),
            "--elf-base" => base = args.next(),
            "--out" => out = args.next().map(PathBuf::from),
            "--list-functions" => list = true,
            _ => {
                eprintln!("unknown {a}");
                std::process::exit(2);
            }
        }
    }
    let (Some(binary), Some(out)) = (binary, out) else {
        eprintln!(
            "usage: run [--list-functions] --binary B [--addresses F] --elf-base 0x.. --out O"
        );
        std::process::exit(2);
    };
    // The library loads ELF sections at their virtual addresses (no rebase),
    // so its addresses are already ELF virtual addresses; --elf-base is only
    // validated.
    if let Some(b) = &base {
        if parse_addr(b).is_none() {
            eprintln!("bad --elf-base {b}");
            std::process::exit(2);
        }
    }
    let r = if list {
        // an empty list is an answer, so a stale file must not pass for one
        let _ = std::fs::remove_file(&out);
        list_mode(&binary, &out)
    } else {
        match addresses {
            Some(a) => decompile_mode(&binary, &a, &out),
            None => Err("--addresses is required".into()),
        }
    };
    if let Err(e) = r {
        eprintln!("ghidra-rs: {e}");
        std::process::exit(1);
    }
    // a timed-out decompiler thread may still be running
    std::process::exit(0);
}
