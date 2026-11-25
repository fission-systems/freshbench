//! Running decompilers: one call per binary, restricted to the selected functions.
//!
//! Every tool sees only the stripped binary and the list of function start
//! addresses (ELF virtual addresses). A tool that rebases the image is given
//! the ELF base so its adapter can translate. Contract for external adapters
//! (`adapters/<tool>/run`):
//!
//! ```text
//! run --binary <stripped> --addresses <file: one 0x... per line> --elf-base 0x... --out <jsonl>
//! ```
//! writing one JSON line per address: `{"address": "0x...", "code": "...", "error": null}`.
//!
//! A call that fails (non-zero exit, timeout, unreadable output) is retried one
//! function at a time, so one bad function costs only itself. What each tool
//! printed is recorded as-is; scoring happens later (`rescore`).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use object::{Object, ObjectSegment};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum Tool {
    /// `fission_cli decomp --json --addresses-file`, run in `cwd` with an optional resource root.
    Fission {
        cli: PathBuf,
        resource_root: Option<PathBuf>,
        function_timeout_ms: u64,
    },
    /// Any adapter following the script contract.
    Script { name: String, command: Vec<String> },
}

impl Tool {
    pub fn name(&self) -> &str {
        match self {
            Tool::Fission { .. } => "fission",
            Tool::Script { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub binary: String,
    pub function: String,
    pub address: u64,
    pub code: Option<String>,
    pub error: Option<String>,
}

/// Lowest PT_LOAD virtual address, page-aligned down (0 for a typical PIE).
pub fn elf_base(data: &[u8]) -> Result<u64> {
    let f = object::File::parse(data)?;
    let lo = f.segments().map(|s| s.address()).min().unwrap_or(0);
    Ok(lo & !0xfff)
}

fn hex_list(addrs: &[u64]) -> String {
    addrs.iter().map(|a| format!("{a:#x}\n")).collect()
}

/// SIGKILL every process of the group `pgid` (the child and whatever it
/// spawned: a shell adapter's JVM or Python stays behind otherwise).
fn kill_group(pgid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = pgid;
}

/// Run `cmd` in its own process group, killing the whole group after
/// `timeout` (and any leftovers once it exits). Returns (exit ok, stdout,
/// stderr tail).
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<(bool, Vec<u8>, String)> {
    let dir = tempfile::tempdir()?;
    let (out_p, err_p) = (dir.path().join("out"), dir.path().join("err"));
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out_p)?)
        .stderr(std::fs::File::create(&err_p)?)
        .spawn()
        .context("spawn")?;
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            kill_group(child.id());
            break Some(s);
        }
        if start.elapsed() > timeout {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = std::fs::read(&out_p)?;
    let err = String::from_utf8_lossy(&std::fs::read(&err_p)?).into_owned();
    let tail = err
        .chars()
        .rev()
        .take(2000)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    match status {
        Some(s) => Ok((s.success(), out, tail)),
        None => Ok((false, out, format!("timeout after {}s", timeout.as_secs()))),
    }
}

/// address -> (code or error) for one call of the tool on `addrs`.
fn call(
    tool: &Tool,
    binary: &Path,
    addrs: &[u64],
    base: u64,
    timeout: Duration,
) -> Result<BTreeMap<u64, Result<String, String>>> {
    let dir = tempfile::tempdir()?;
    let af = dir.path().join("addrs.txt");
    std::fs::write(&af, hex_list(addrs))?;
    match tool {
        Tool::Fission {
            cli,
            resource_root,
            function_timeout_ms,
        } => {
            let mut c = Command::new(cli);
            c.arg("--no-db").arg("decomp").arg(binary).args([
                "--json",
                "--no-header",
                "--no-warnings",
            ]);
            c.arg("--addresses-file")
                .arg(&af)
                .arg("--timeout-ms")
                .arg(function_timeout_ms.to_string());
            if let Some(r) = resource_root {
                c.arg("--resource-root").arg(r);
            }
            c.env("FISSION_BENCHMARK_NO_CACHE", "1");
            let (ok, out, err) = run_with_timeout(c, timeout)?;
            if !ok {
                bail!("fission_cli failed: {err}");
            }
            let v: serde_json::Value =
                serde_json::from_slice(&out).context("fission_cli output is not JSON")?;
            let items = match &v {
                serde_json::Value::Array(a) => a.clone(),
                serde_json::Value::Object(o) => o
                    .get("functions")
                    .or_else(|| o.get("results"))
                    .and_then(|x| x.as_array().cloned())
                    .unwrap_or_default(),
                _ => vec![],
            };
            let mut m = BTreeMap::new();
            for it in items {
                let Some(a) = it
                    .get("address")
                    .and_then(|a| a.as_str())
                    .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                else {
                    continue;
                };
                match it.get("code").and_then(|c| c.as_str()) {
                    Some(code) => m.insert(a, Ok(code.to_string())),
                    None => m.insert(
                        a,
                        Err(it
                            .get("error")
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "no code".into())),
                    ),
                };
            }
            Ok(m)
        }
        Tool::Script { command, .. } => {
            let outf = dir.path().join("out.jsonl");
            let mut c = Command::new(&command[0]);
            c.args(&command[1..])
                .arg("--binary")
                .arg(binary)
                .arg("--addresses")
                .arg(&af);
            c.arg("--elf-base")
                .arg(format!("{base:#x}"))
                .arg("--out")
                .arg(&outf);
            let (ok, _, err) = run_with_timeout(c, timeout)?;
            if !ok {
                bail!("adapter failed: {err}");
            }
            #[derive(Deserialize)]
            struct Line {
                address: String,
                code: Option<String>,
                error: Option<String>,
            }
            let mut m = BTreeMap::new();
            for line in std::fs::read_to_string(&outf)
                .context("adapter wrote no output")?
                .lines()
            {
                if line.trim().is_empty() {
                    continue;
                }
                let l: Line = serde_json::from_str(line)?;
                let a = u64::from_str_radix(l.address.trim_start_matches("0x"), 16)?;
                m.insert(
                    a,
                    match l.code {
                        Some(c) if !c.trim().is_empty() => Ok(c),
                        _ => Err(l.error.unwrap_or_else(|| "no code".into())),
                    },
                );
            }
            Ok(m)
        }
    }
}

/// Decompile `funcs` (address -> name) of one binary; failed calls are retried per function.
pub fn decompile_binary(
    tool: &Tool,
    binary: &Path,
    binary_name: &str,
    funcs: &BTreeMap<u64, String>,
    binary_timeout: Duration,
    function_timeout: Duration,
) -> Result<Vec<Record>> {
    let data = std::fs::read(binary)?;
    let base = elf_base(&data)?;
    let addrs: Vec<u64> = funcs.keys().copied().collect();
    let mut got: BTreeMap<u64, Result<String, String>> =
        match call(tool, binary, &addrs, base, binary_timeout) {
            Ok(m) => m,
            Err(e) => {
                let whole = format!("{e:#}");
                let mut m = BTreeMap::new();
                for a in &addrs {
                    let r = match call(tool, binary, &[*a], base, function_timeout) {
                        Ok(mut one) => one
                            .remove(a)
                            .unwrap_or_else(|| Err("missing from output".into())),
                        Err(e1) => Err(format!("{e1:#} (whole-binary call: {whole})")),
                    };
                    m.insert(*a, r);
                }
                m
            }
        };
    Ok(funcs
        .iter()
        .map(|(a, name)| {
            let r = got
                .remove(a)
                .unwrap_or_else(|| Err("missing from output".into()));
            let (code, error) = match r {
                Ok(c) => (Some(c), None),
                Err(e) => (None, Some(e)),
            };
            Record {
                binary: binary_name.to_string(),
                function: name.clone(),
                address: *a,
                code,
                error,
            }
        })
        .collect())
}

/// The functions `tool`'s own analysis finds in `binary`, no addresses given
/// (`adapters/README.md`, list mode): `(start, end)` with `end` when known.
pub fn list_functions(
    tool: &Tool,
    binary: &Path,
    timeout: Duration,
) -> Result<Vec<(u64, Option<u64>)>> {
    let hex = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16);
    match tool {
        Tool::Fission {
            cli, resource_root, ..
        } => {
            let mut c = Command::new(cli);
            c.arg("--no-db").arg("list").arg(binary).arg("--json");
            if let Some(r) = resource_root {
                c.arg("--resource-root").arg(r);
            }
            let (ok, out, err) = run_with_timeout(c, timeout)?;
            if !ok {
                bail!("fission_cli failed: {err}");
            }
            let v: Vec<serde_json::Value> =
                serde_json::from_slice(&out).context("fission_cli output is not JSON")?;
            let mut found = Vec::new();
            for it in v {
                let flag = |k: &str| it.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
                // imports and thunks are not functions of the binary
                if flag("is_import") || flag("is_thunk_like") {
                    continue;
                }
                let Some(a) = it.get("address").and_then(|a| a.as_str()) else {
                    continue;
                };
                let start = hex(a)?;
                let size = it.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
                found.push((start, (size > 0).then(|| start + size)));
            }
            Ok(found)
        }
        Tool::Script { command, .. } => {
            let dir = tempfile::tempdir()?;
            let outf = dir.path().join("functions.jsonl");
            let data = std::fs::read(binary)?;
            let mut c = Command::new(&command[0]);
            c.args(&command[1..])
                .arg("--list-functions")
                .arg("--binary")
                .arg(binary)
                .arg("--elf-base")
                .arg(format!("{:#x}", elf_base(&data)?))
                .arg("--out")
                .arg(&outf);
            let (ok, _, err) = run_with_timeout(c, timeout)?;
            if !ok {
                bail!("adapter failed: {err}");
            }
            #[derive(Deserialize)]
            struct Line {
                start: String,
                end: Option<String>,
            }
            let mut found = Vec::new();
            for line in std::fs::read_to_string(&outf)
                .context("adapter wrote no output")?
                .lines()
            {
                if line.trim().is_empty() {
                    continue;
                }
                let l: Line = serde_json::from_str(line)?;
                found.push((hex(&l.start)?, l.end.as_deref().map(hex).transpose()?));
            }
            Ok(found)
        }
    }
}

/// Why a function has no code, from the error text this module and the
/// adapters write: `timeout`, `crash` (the tool exited non-zero), `missing`
/// (the tool returned nothing for the address), `adapter` (unreadable
/// output) or `tool_error` (the tool reported an error for the function).
pub fn failure_kind(error: &str) -> &'static str {
    if error.contains("timeout after") {
        "timeout"
    } else if error.contains("missing from output") {
        "missing"
    } else if error.contains("adapter wrote no output") || error.contains("output is not JSON") {
        "adapter"
    } else if error.starts_with("adapter failed") || error.starts_with("fission_cli failed") {
        "crash"
    } else {
        "tool_error"
    }
}

/// Write records as gzip JSON lines.
pub fn write_records(path: &Path, recs: &[Record]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let f = std::fs::File::create(path)?;
    let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    for r in recs {
        serde_json::to_writer(&mut gz, r)?;
        gz.write_all(b"\n")?;
    }
    gz.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_kinds() {
        assert_eq!(
            failure_kind("adapter failed: timeout after 300s"),
            "timeout"
        );
        assert_eq!(
            failure_kind("adapter failed: Segmentation fault (whole-binary call: x)"),
            "crash"
        );
        assert_eq!(failure_kind("missing from output"), "missing");
        assert_eq!(failure_kind("fission_cli output is not JSON"), "adapter");
        assert_eq!(failure_kind("no function at address"), "tool_error");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_the_whole_process_group() {
        let d = tempfile::tempdir().unwrap();
        let pidf = d.path().join("pid");
        let mut c = Command::new("sh");
        // a grandchild the shell does not wait for, like an adapter's JVM
        c.arg("-c")
            .arg(format!("sleep 30 & echo $! > {}; wait", pidf.display()));
        let (ok, _, err) = run_with_timeout(c, Duration::from_millis(500)).unwrap();
        assert!(!ok && err.starts_with("timeout"));
        let pid = std::fs::read_to_string(&pidf).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "grandchild {} survived the timeout", pid.trim());
    }

    #[test]
    fn timeout_kills() {
        let mut c = Command::new("sh");
        c.args(["-c", "echo hi; sleep 5"]);
        let t = Instant::now();
        let (ok, out, err) = run_with_timeout(c, Duration::from_millis(300)).unwrap();
        assert!(!ok && err.starts_with("timeout"));
        assert_eq!(out, b"hi\n");
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn script_contract_and_per_function_retry() {
        // a fake adapter: fails when given more than one address, else echoes a function
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("run");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while [ $# -gt 0 ]; do case "$1" in --addresses) A="$2"; shift 2;; --out) O="$2"; shift 2;; *) shift 2;; esac; done
n=$(wc -l < "$A")
[ "$n" -gt 1 ] && exit 3
a=$(cat "$A")
if [ "$a" = "0x20" ]; then echo "{\"address\":\"$a\",\"code\":null,\"error\":\"boom\"}" > "$O";
else echo "{\"address\":\"$a\",\"code\":\"int f(void){return 1;}\",\"error\":null}" > "$O"; fi
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // any ELF-or-not file: elf_base needs a parsable object, so use this test binary itself
        let me = std::env::current_exe().unwrap();
        let tool = Tool::Script {
            name: "fake".into(),
            command: vec![script.to_string_lossy().into_owned()],
        };
        let funcs: BTreeMap<u64, String> =
            [(0x10, "a".to_string()), (0x20, "b".to_string())].into();
        let recs = decompile_binary(
            &tool,
            &me,
            "me",
            &funcs,
            Duration::from_secs(10),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].code.as_deref(), Some("int f(void){return 1;}"));
        assert_eq!(recs[1].code, None);
        assert_eq!(recs[1].error.as_deref(), Some("boom"));
    }
}
