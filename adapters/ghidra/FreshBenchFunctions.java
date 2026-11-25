// FreshBench adapter script, list mode: the functions Ghidra's auto-analysis
// found, as ELF virtual addresses.
// args: <elf base> <out jsonl>
//@category FreshBench
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import java.io.FileWriter;
import java.io.PrintWriter;

public class FreshBenchFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        long elfBase = Long.decode(a[0]);
        // the inverse of FreshBenchDecompile's mapping:
        // ELF address = program address - image base + lowest ELF load address
        long imageBase = currentProgram.getImageBase().getOffset();
        try (PrintWriter out = new PrintWriter(new FileWriter(a[1]))) {
            FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
            while (it.hasNext()) {
                Function f = it.next();
                // imports (EXTERNAL space) and the PLT stubs / jump thunks that forward to them
                if (f.isExternal() || f.isThunk()) continue;
                Address entry = f.getEntryPoint();
                if (!entry.isLoadedMemoryAddress()) continue;
                long start = entry.getOffset() - imageBase + elfBase;
                String end = "null";
                AddressSetView body = f.getBody();
                if (body != null && !body.isEmpty()) {
                    Address max = body.getMaxAddress();
                    if (max.getAddressSpace().equals(entry.getAddressSpace())) {
                        end = String.format("\"0x%x\"", max.getOffset() + 1 - imageBase + elfBase);
                    }
                }
                out.println(String.format("{\"start\":\"0x%x\",\"end\":%s}", start, end));
            }
        }
    }
}
