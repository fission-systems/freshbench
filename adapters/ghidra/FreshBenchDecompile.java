// FreshBench adapter script: decompile the functions at the given ELF addresses.
// args: <addresses file> <elf base> <out jsonl>
//@category FreshBench
import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.decompiler.DecompInterface;
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
        long elfBase = Long.decode(a[1]);
        // Ghidra may load a position-independent image at its own base:
        // program address = image base + (ELF address - lowest ELF load address)
        long imageBase = currentProgram.getImageBase().getOffset();
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
                String code = null, error = null;
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
                        if (r != null && r.decompileCompleted() && r.getDecompiledFunction() != null) {
                            code = r.getDecompiledFunction().getC();
                        } else {
                            error = r == null ? "no result" : r.getErrorMessage();
                        }
                    }
                } catch (Exception e) {
                    error = e.toString();
                }
                out.println("{\"address\":" + json(line) + ",\"code\":" + json(code) + ",\"error\":" + json(error) + "}");
            }
        }
        dec.dispose();
    }
}
