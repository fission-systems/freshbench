"""borer decompiler backend for DecBench (FreshBench addition; not upstream).

borer (https://github.com/imjustprism/borer) ships a standalone Rust CLI. Like
the kuna backend this shells out once per binary and decompiles every function
from one load:

    borer export <binary> --c <out.c> --library

The export writes each function as

    // <name> @ 0x<entry>[: ...]
    <prototype>
    {
      ...
    }

after a shared prelude of includes, types and globals. Functions over the
export's block budget are written as a ``cut`` comment; those are decompiled
alone with ``borer decompile <binary> <addr> full 0 <big>`` (whose text is the
body with the prototype in a ``// signature:`` comment) and wrapped the same
way the export wraps them. A ``does not decompile`` comment is a failure.

borer emits no variable or line-address records, so type_match takes DecBench's
legacy text path for it (``parse_c_variables``), as for every producer outside
``ADDRESS_CORRESPONDENCE_BACKENDS``.

Addresses: the driver asks for DWARF ``low_pc`` values (ELF link space). If
borer reports entries in another space (a load base), the constant offset is
recovered as the delta that maps the most borer entries onto requested
addresses; the delta is recorded in the result's metadata.

Locate the CLI via ``$BORER_BIN`` or ``borer`` on ``$PATH``.
"""

from __future__ import annotations

import collections
import contextlib
import logging
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

from decbench.decompilers.base import Decompiler, DecompilerConfig
from decbench.decompilers.raw import common
from decbench.decompilers.registry import register_decompiler
from decbench.models.decompilation import (
    DecompilationResult,
    DecompilerMetadata,
    FunctionDecompilation,
)

_l = logging.getLogger(__name__)

_HEADER = re.compile(r"^// (\S+) @ (0x[0-9a-fA-F]+)(?::\s*(.*))?$")
_ENV = {"BORER_NO_UPDATE_CHECK": "1"}


def parse_export(text: str) -> dict[int, tuple[str, str | None, str]]:
    """entry -> (name, code or None, status) from an ``export --c`` file.

    status is ``ok``, ``cut`` or ``failed``.
    """
    out: dict[int, tuple[str, str | None, str]] = {}
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        m = _HEADER.match(lines[i])
        if not m:
            i += 1
            continue
        name, entry, rest = m.group(1), int(m.group(2), 16), (m.group(3) or "")
        if rest.startswith("cut, not decompiled"):
            out[entry] = (name, None, "cut")
            i += 1
            continue
        if rest.startswith("does not decompile"):
            out[entry] = (name, None, "failed")
            i += 1
            continue
        # prototype, then "{", then the body up to a "}" in column 0
        j = i + 1
        if j + 1 < len(lines) and lines[j + 1] == "{" and lines[j] and not lines[j].startswith("//"):
            k = j + 2
            while k < len(lines) and lines[k] != "}":
                k += 1
            if k < len(lines):
                out[entry] = (name, "\n".join(lines[j : k + 1]) + "\n", "ok")
                i = k + 1
                continue
        out[entry] = (name, None, "failed")
        i += 1
    return out


def wrap_single(text: str, name: str) -> str | None:
    """A ``decompile`` text (comments, declarations, statements) as a definition."""
    lines = text.splitlines()
    comments = 0
    while comments < len(lines) and lines[comments].startswith("//"):
        comments += 1
    head = next(
        (l[len("// signature: "):].strip() for l in lines[:comments] if l.startswith("// signature: ")),
        f"void {name}(void)",
    )
    body = lines[comments:]
    if not any(l.strip() for l in body):
        return None
    return head + "\n{\n" + "".join(f"  {l}\n" if l else "\n" for l in body) + "}\n"


def best_delta(entries: list[int], targets: set[int]) -> tuple[int, int]:
    """The constant d maximising |{e + d} & targets| (ties prefer 0), and that count."""
    if not targets or not entries:
        return 0, 0
    exact = sum(1 for e in entries if e in targets)
    votes: collections.Counter[int] = collections.Counter()
    tsample = sorted(targets)[:300]
    for e in sorted(entries)[:300]:
        for t in tsample:
            votes[t - e] += 1
    best, best_hits = 0, exact
    for d, _ in votes.most_common(5):
        hits = sum(1 for e in entries if e + d in targets)
        if hits > best_hits:
            best, best_hits = d, hits
    return best, best_hits


@register_decompiler("borer")
class RawBorerDecompiler(Decompiler):
    """borer (pure-Rust reverse-engineering tool) driven via its CLI."""

    name = "borer"
    display_name = "borer"

    def __init__(self, config: DecompilerConfig | None = None):
        super().__init__(config)

    @staticmethod
    def _bin() -> str | None:
        env = os.environ.get("BORER_BIN")
        if env and Path(env).is_file():
            return env
        return shutil.which("borer")

    def is_available(self) -> bool:
        return self._bin() is not None

    def get_version(self) -> str | None:
        b = self._bin()
        if not b:
            return None
        try:
            p = subprocess.run([b, "--version"], capture_output=True, text=True, timeout=30,
                               env={**os.environ, **_ENV})
            out = (p.stdout or p.stderr or "").strip()
            m = re.search(r"(\d+\.\d+(?:\.\d+)?\S*)", out)
            return m.group(1) if m else (out.splitlines()[0] if out else "unknown")
        except Exception:  # noqa: BLE001
            return "unknown"

    def _timeout(self) -> float | None:
        env = os.environ.get("DECBENCH_BORER_TIMEOUT")
        if env:
            with contextlib.suppress(ValueError):
                return int(env)
        return self.config.binary_timeout_seconds

    def _run(self, cmd: list[str], timeout: float | None) -> tuple[int, str, str]:
        p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             start_new_session=True, env={**os.environ, **_ENV})
        try:
            so, se = p.communicate(timeout=timeout)
        finally:
            if p.poll() is None:
                try:
                    os.killpg(os.getpgid(p.pid), signal.SIGKILL)
                except (ProcessLookupError, PermissionError, OSError):
                    with contextlib.suppress(Exception):
                        p.kill()
                with contextlib.suppress(Exception):
                    p.wait(timeout=15)
        return p.returncode, so or "", se or ""

    def decompile_binary(
        self,
        binary_path: Path,
        functions: list[tuple[str, int]] | None = None,
        output_dir: Path | None = None,
        function_names: set[str] | None = None,
        progress_path: Path | None = None,
    ) -> DecompilationResult:
        if not self.is_available():
            raise RuntimeError("borer CLI not found; set $BORER_BIN or add it to PATH")
        start = time.time()
        b = self._bin()
        text_range = common.elf_text_ranges(binary_path)
        targets = common.addr_targets_of(function_names)
        decompiled: dict[str, FunctionDecompilation] = {}
        failed: list[str] = []
        extra: dict[str, Any] = {"backend": "borer", "via": "raw"}
        timed_out = False

        def result() -> DecompilationResult:
            return DecompilationResult(
                binary_path=binary_path, binary_name=binary_path.stem,
                decompiler=DecompilerMetadata(
                    decompiler_name=self.id, decompiler_version=self.get_version(),
                    total_time_seconds=time.time() - start, timeout_occurred=timed_out,
                    failed_functions=list(failed), extra=extra),
                functions=decompiled, output_dir=output_dir)

        with tempfile.TemporaryDirectory(prefix="borer-") as td:
            out_c = Path(td) / "out.c"
            try:
                rc, _so, se = self._run([b, "export", str(binary_path), "--c", str(out_c), "--library"],
                                        self._timeout())
            except subprocess.TimeoutExpired:
                timed_out = True
                failed.append("all"); extra["error"] = "timeout"
                return result()
            if not out_c.exists():
                failed.append("all"); extra["error"] = f"export exited {rc}: {se[-500:]}"
                return result()
            parsed = parse_export(out_c.read_text(errors="replace"))

        delta, hits = best_delta(list(parsed), targets)
        extra.update({"address_delta": delta, "exported": len(parsed),
                      "cut": sum(1 for v in parsed.values() if v[2] == "cut"),
                      "export_failed": sum(1 for v in parsed.values() if v[2] == "failed")})
        enumerated = sorted(
            ((v[0], e + delta) for e, v in parsed.items()
             if not common.should_skip_function(v[0], e + delta, text_range, targets)),
            key=lambda x: x[1])
        if functions is not None:
            requested = {n for (n, _a) in functions}
            enumerated = [(n, a) for (n, a) in enumerated if n in requested]
        enumerated = common.narrow_to_source(enumerated, targets, backend="borer",
                                             binary_name=binary_path.name)
        for name, addr in enumerated:
            entry = addr - delta
            _n, code, status = parsed[entry]
            if status == "cut":
                try:
                    rc, so, _se = self._run([b, "decompile", str(binary_path), hex(entry), "full", "0",
                                             "100000000"], self._timeout())
                    code = wrap_single(so, name) if rc == 0 else None
                except subprocess.TimeoutExpired:
                    code = None
            if not code:
                failed.append(name)
                continue
            decompiled[name] = FunctionDecompilation(
                name=name, address=addr, decompiled_code=code, line_count=code.count("\n") + 1,
                line_mappings=[], variables=[], metadata=common.extract_metrics(code))
        res = result()
        if output_dir:
            output_dir.mkdir(parents=True, exist_ok=True)
            res.to_c_file(output_dir / f"{self.name}_{binary_path.stem}.c")
            res.to_toml(output_dir / f"{self.name}_{binary_path.stem}.toml")
        return res
