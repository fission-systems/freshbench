// FreshBench adapter script: decompile the functions at the given ELF addresses.
// args: <addresses file> <elf base> <out jsonl>
//
// The C text is rendered here from Ghidra's token lines (DecompilerUtils.toLines),
// so line N of `code` is exactly token line N, and every line carries the
// instruction addresses of its tokens (`line_mappings`). Variables come from the
// HighFunction's symbols: parameters with their ordinal (`arg_index`), stack
// symbols with their offset, each with the addresses of the tokens that use it.
//@category FreshBench
import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.decompiler.ClangLine;
import ghidra.app.decompiler.ClangToken;
import ghidra.app.decompiler.ClangTokenGroup;
import ghidra.app.decompiler.ClangVariableToken;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.component.DecompilerUtils;
import ghidra.program.model.pcode.HighFunction;
import ghidra.program.model.pcode.HighSymbol;
import ghidra.program.model.pcode.HighVariable;
import ghidra.program.model.listing.VariableStorage;
import ghidra.program.model.data.DataType;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.lang.Register;
import ghidra.program.model.listing.Function;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.math.BigInteger;
import java.nio.file.Files;
import java.nio.file.Paths;

public class FreshBenchDecompile extends GhidraScript {
    private long imageBase, elfBase;

    private Long elf(Address a) {
        if (a == null) return null;
        long v = a.getOffset() - imageBase + elfBase;
        return v >= 0 ? v : null;
    }

    private static String addrs(TreeSet<Long> s) {
        StringBuilder b = new StringBuilder("[");
        for (Long v : s) { if (b.length() > 1) b.append(','); b.append(v); }
        return b.append(']').toString();
    }

    /** code, line_mappings, variables and return_type of one decompiled function, as JSON fields. */
    private String render(DecompileResults r) {
        ClangTokenGroup markup = r.getCCodeMarkup();
        HighFunction hf = r.getHighFunction();
        List<ClangLine> lines = DecompilerUtils.toLines(markup);
        StringBuilder code = new StringBuilder();
        StringBuilder lm = new StringBuilder("[");
        Map<HighSymbol, TreeSet<Long>> uses = new LinkedHashMap<>();
        int n = 0;
        for (ClangLine line : lines) {
            n++;
            StringBuilder text = new StringBuilder();
            for (int i = 0; i < line.getIndent(); i++) text.append(' ');
            TreeSet<Long> at = new TreeSet<>();
            for (ClangToken t : line.getAllTokens()) {
                text.append(t.getText());
                Long a = elf(t.getMinAddress());
                if (a != null) at.add(a);
                if (t instanceof ClangVariableToken) {
                    HighVariable hv = ((ClangVariableToken) t).getHighVariable();
                    HighSymbol hs = hv == null ? null : hv.getSymbol();
                    if (hs != null && a != null) uses.computeIfAbsent(hs, k -> new TreeSet<>()).add(a);
                }
            }
            code.append(text).append('\n');
            if (!at.isEmpty()) {
                if (lm.length() > 1) lm.append(',');
                lm.append("{\"line_number\":").append(n).append(",\"addresses\":").append(addrs(at)).append('}');
            }
        }
        lm.append(']');
        StringBuilder vars = new StringBuilder("[");
        String ret = null;
        if (hf != null) {
            Iterator<HighSymbol> it = hf.getLocalSymbolMap().getSymbols();
            while (it.hasNext()) {
                HighSymbol hs = it.next();
                DataType dt = hs.getDataType();
                VariableStorage st = hs.getStorage();
                boolean stack = st != null && st.isStackStorage();
                String kind = hs.isParameter() ? "arg" : (stack ? "stack" : "register");
                if (vars.length() > 1) vars.append(',');
                vars.append("{\"name\":").append(json(hs.getName()))
                    .append(",\"type\":").append(json(dt == null ? "" : dt.getDisplayName()))
                    .append(",\"kind\":").append(json(kind))
                    .append(",\"arg_index\":").append(hs.isParameter() ? String.valueOf(hs.getCategoryIndex()) : "null")
                    .append(",\"stack_offset\":").append(stack ? String.valueOf(st.getStackOffset()) : "null")
                    .append(",\"size\":").append(hs.getSize())
                    .append(",\"addresses\":").append(addrs(uses.getOrDefault(hs, new TreeSet<>())))
                    .append('}');
            }
            DataType rt = hf.getFunctionPrototype().getReturnType();
            ret = rt == null ? null : rt.getDisplayName();
        }
        vars.append(']');
        return ",\"code\":" + json(code.toString()) + ",\"line_mappings\":" + lm + ",\"variables\":" + vars
            + ",\"return_type\":" + json(ret);
    }

    private static String json(String s) {
        if (s == null) return "null";
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"': b.append("\\\""); break;
                case '\\': b.append("\\\\"); break;
                case '\n': b.append("\\n"); break;
                case '\r': b.append("\\r"); break;
                case '\t': b.append("\\t"); break;
                default:
                    if (c < 0x20) b.append(String.format("\\u%04x", (int) c)); else b.append(c);
            }
        }
        return b.append('"').toString();
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        elfBase = Long.decode(a[1]);
        // Ghidra may load a position-independent image at its own base:
        // program address = image base + (ELF address - lowest ELF load address)
        imageBase = currentProgram.getImageBase().getOffset();
        Register tmode = currentProgram.getLanguage().getRegister("TMode");
        DecompInterface dec = new DecompInterface();
        dec.setOptions(new DecompileOptions());
        dec.openProgram(currentProgram);
        int timeout = Integer.parseInt(System.getenv().getOrDefault("GHIDRA_FUNCTION_TIMEOUT", "300"));
        try (PrintWriter out = new PrintWriter(new FileWriter(a[2]))) {
            for (String line : Files.readAllLines(Paths.get(a[0]))) {
                line = line.trim();
                if (line.isEmpty()) continue;
                long req = Long.decode(line);
                boolean thumb = (req & 1) == 1 && tmode != null;
                long va = thumb ? req & ~1L : req;
                String fields = null, error = null;
                try {
                    Address addr = toAddr(imageBase + (va - elfBase));
                    Function f = getFunctionAt(addr);
                    if (f == null) {
                        // the tool's analysis did not create it: create it at the given start
                        if (thumb) {
                            currentProgram.getProgramContext().setValue(tmode, addr, addr, BigInteger.ONE);
                        }
                        new DisassembleCommand(addr, null, true).applyTo(currentProgram, monitor);
                        f = createFunction(addr, null);
                    }
                    if (f == null) {
                        error = "no function at " + addr;
                    } else {
                        DecompileResults r = dec.decompileFunction(f, timeout, monitor);
                        if (r != null && r.decompileCompleted() && r.getCCodeMarkup() != null) {
                            fields = render(r);
                        } else {
                            error = r == null ? "no result" : r.getErrorMessage();
                        }
                    }
                } catch (Exception e) {
                    error = e.toString();
                }
                out.println("{\"address\":" + json(line)
                    + (fields != null ? fields : ",\"code\":null") + ",\"error\":" + json(error) + "}");
            }
        }
        dec.dispose();
    }
}
