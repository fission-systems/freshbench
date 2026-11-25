//! Opt-in error reporting to Sentry, compiled only with the `sentry` feature.
//!
//! Off by default: nothing is sent unless `FRESHBENCH_SENTRY=1` and
//! `SENTRY_DSN` are both set. Only panics are reported, including a scorer
//! panic that `dataset` catches so that it costs one row.
//!
//! No free text leaves the process. A panic message, an exception value, a
//! message or a log entry can hold anything the process was looking at
//! (source text, decompiler output, paths, credentials), so none of them is
//! sent; they are replaced by a template built from the fields below. What an
//! event can carry is decided here, by [`scrub`], not by the SDK:
//!
//! * the exception type (`panic`) and mechanism;
//! * the panic location, as `file:line`, when the file is FreshBench's own
//!   source (`crates/...`), the Rust standard library (`library/...`) or a
//!   crates.io dependency (`deps/<crate>-<version>/...`); any other path is
//!   dropped (see [`clean_source_path`]);
//! * stack frames: the function name when its crate is FreshBench's, the
//!   standard library's or Rayon's (others become `<other>`), `in_app`, and
//!   file and line under the same rule as the location;
//! * fingerprint `[exception type, location]`, the `location` tag, level,
//!   timestamp and event id;
//! * release (`freshbench@<version>`), environment, platform and SDK name
//!   only from fixed forms or lists, and the SDK version.
//!
//! Everything else is dropped: other tags, extra, contexts, breadcrumbs,
//! threads, loaded modules, debug images, request, user, server name,
//! transaction, logger, and each frame's module, package, absolute path,
//! symbol, addresses, locals and source context.

use std::borrow::Cow;
use std::panic::PanicHookInfo;

use sentry::integrations::backtrace::{current_stacktrace, ProcessStacktraceIntegration};
use sentry::integrations::panic::PanicIntegration;
use sentry::protocol::{
    ClientSdkInfo, Event, Exception, Frame, Level, LogEntry, Mechanism, Stacktrace, Values,
};

/// Keeps the Sentry client alive. `main` returns an `ExitCode` instead of
/// calling `process::exit`, so this guard is dropped on every exit path and
/// its drop flushes queued events; the panic handler flushes on its own.
pub type Guard = sentry::ClientInitGuard;

/// What replaces a message, log entry or exception value that has no template.
const REDACTED: &str = "[redacted]";
/// The only tag an event keeps: the panic location, set by [`panic_event`].
const LOCATION_TAG: &str = "location";
/// Fingerprint part for an event without a clean location.
const NO_LOCATION: &str = "<unknown location>";
/// Function name of a frame outside [`FRAME_CRATES`].
const OTHER_FRAME: &str = "<other>";
/// Crates whose function names a stack frame may carry.
const FRAME_CRATES: &[&str] = &[
    "freshbench",
    "rust_joern",
    "std",
    "core",
    "alloc",
    "panic_unwind",
    "panic_abort",
    "rayon",
    "rayon_core",
];
/// First component of every path [`clean_source_path`] returns.
const SOURCE_ROOTS: &[&str] = &["crates", "library", "deps"];
/// FreshBench's own crates, as workspace-relative directories.
const OWN_CRATES: &[&str] = &["crates/freshbench/", "crates/rust-joern/"];
/// Exception and mechanism types an event may carry (`panic` is the only one
/// FreshBench produces; anything else is reported as `error` / `generic`).
const EXCEPTION_TYPES: &[&str] = &["panic", "error"];
const MECHANISM_TYPES: &[&str] = &["panic", "generic"];
const PLATFORMS: &[&str] = &["native", "rust", "other"];
const ENVIRONMENTS: &[&str] = &[
    "production",
    "development",
    "staging",
    "ci",
    "test",
    "local",
];
const SDK_NAMES: &[&str] = &["sentry.rust"];

/// The DSN to report to, or `None` when reporting is not enabled.
fn dsn_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    if lookup("FRESHBENCH_SENTRY").as_deref() != Some("1") {
        return None;
    }
    lookup("SENTRY_DSN")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Start reporting if the environment asks for it. Call once, from a binary's `main`.
pub fn init() -> Option<Guard> {
    let dsn = dsn_from(|key| std::env::var(key).ok())?;
    if dsn.parse::<sentry::types::Dsn>().is_err() {
        eprintln!("telemetry: SENTRY_DSN is not a valid DSN; reporting stays off");
        return None;
    }
    Some(sentry::init(options(&dsn, |key| std::env::var(key).ok())))
}

/// Client options: only the panic and stack-trace integrations (no contexts:
/// they would collect OS, device and host details only for [`scrub`] to
/// drop), and [`scrub`] as the last step before an event is sent.
fn options(dsn: &str, lookup: impl Fn(&str) -> Option<String>) -> sentry::ClientOptions {
    sentry::ClientOptions::new()
        .dsn(dsn)
        .release(release(&lookup))
        .environment(environment(&lookup))
        .default_integrations(false)
        .add_integration(PanicIntegration::new().add_extractor(panic_event))
        .add_integration(ProcessStacktraceIntegration)
        .send_default_pii(false)
        .sample_rate(1.0)
        .traces_sample_rate(0.0)
        .attach_stacktrace(false)
        .max_breadcrumbs(0)
        .before_send(|event| Some(scrub(event)))
}

/// Release: SENTRY_RELEASE, then FRESHBENCH_RELEASE, then `freshbench@<version>`.
/// A value [`clean_release`] rejects falls through to the next.
fn release(lookup: impl Fn(&str) -> Option<String>) -> Cow<'static, str> {
    ["SENTRY_RELEASE", "FRESHBENCH_RELEASE"]
        .iter()
        .filter_map(|key| lookup(key))
        .find_map(|v| clean_release(v.trim()).map(str::to_owned))
        .map(Cow::Owned)
        .unwrap_or_else(|| sentry::release_name!().unwrap_or(Cow::Borrowed("freshbench@unknown")))
}

/// Environment: SENTRY_ENVIRONMENT when it is one of [`ENVIRONMENTS`], else `production`.
fn environment(lookup: impl Fn(&str) -> Option<String>) -> Cow<'static, str> {
    lookup("SENTRY_ENVIRONMENT")
        .and_then(|v| clean_environment(v.trim()))
        .map_or(Cow::Borrowed("production"), Cow::Borrowed)
}

/// The event for a panic. The panic message is never read: it is formatted
/// from whatever the panicking code had (a function's source, a tool's
/// output, a path). The location comes from the compiler, not from data.
fn panic_event(info: &PanicHookInfo<'_>) -> Option<Event<'static>> {
    let mut event = Event {
        level: Level::Fatal,
        ..Default::default()
    };
    event.exception = Values::from(vec![Exception {
        ty: "panic".into(),
        value: None,
        mechanism: Some(Mechanism {
            ty: "panic".into(),
            handled: Some(false),
            ..Default::default()
        }),
        stacktrace: current_stacktrace(),
        ..Default::default()
    }]);
    if let Some(location) = info
        .location()
        .and_then(|l| clean_location(l.file(), l.line()))
    {
        event.tags.insert(LOCATION_TAG.into(), location);
    }
    Some(event)
}

/// `path:line` for a source file [`clean_source_path`] accepts.
fn clean_location(file: &str, line: u32) -> Option<String> {
    clean_source_path(file).map(|p| format!("{p}:{line}"))
}

/// A source path as one of three public forms, or `None`:
///
/// * FreshBench's own source, workspace-relative (`crates/freshbench/src/x.rs`);
///   rustc records workspace members relative, and an absolute path is
///   reduced to the part from `crates/` on;
/// * the standard library (`/rustc/<commit>/library/...` -> `library/...`);
/// * a crates.io dependency (`.../registry/src/<index>/<crate>-<ver>/...` ->
///   `deps/<crate>-<ver>/...`).
///
/// Anything else (a home directory, a git dependency, a corpus file, a
/// Windows drive or UNC path outside those forms) is dropped, and so is a
/// result with any character outside `[A-Za-z0-9_./-]`.
fn clean_source_path(raw: &str) -> Option<String> {
    let p = raw.replace('\\', "/");
    let candidate = if let Some(rest) = p.strip_prefix("/rustc/") {
        rest.split_once('/')?.1.to_owned()
    } else if let Some((_, rest)) = p.split_once("/registry/src/") {
        format!("deps/{}", rest.split_once('/')?.1)
    } else if OWN_CRATES.iter().any(|c| p.starts_with(c)) {
        p
    } else {
        let i = OWN_CRATES
            .iter()
            .filter_map(|c| p.find(&format!("/{c}")))
            .min()?;
        p[i + 1..].to_owned()
    };
    is_clean_source_path(&candidate).then_some(candidate)
}

fn is_clean_source_path(p: &str) -> bool {
    let mut parts = p.split('/');
    p.len() <= 200
        && p.ends_with(".rs")
        && parts
            .next()
            .is_some_and(|root| SOURCE_ROOTS.contains(&root))
        && parts.all(|c| !c.is_empty() && c != "." && c != "..")
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-'))
}

fn is_clean_location(location: &str) -> bool {
    location.rsplit_once(':').is_some_and(|(path, line)| {
        is_clean_source_path(path)
            && !line.is_empty()
            && line.len() <= 10
            && line.bytes().all(|b| b.is_ascii_digit())
    })
}

/// `s` when it is one of `allowed`, as the `'static` entry.
fn one_of(s: &str, allowed: &[&'static str]) -> Option<&'static str> {
    allowed.iter().copied().find(|a| *a == s)
}

fn clean_environment(s: &str) -> Option<&'static str> {
    one_of(s, ENVIRONMENTS)
}

/// A release name `freshbench@<version>[+build]`, or `None`.
fn clean_release(s: &str) -> Option<&str> {
    let version = s.strip_prefix("freshbench@")?;
    (!version.is_empty()
        && version.len() <= 100
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'+')))
    .then_some(s)
}

/// A version such as the SDK's `0.49.6`, or `None`.
fn clean_version(s: &str) -> Option<&str> {
    (!s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+')))
    .then_some(s)
}

/// The crate a demangled function name starts with: `<freshbench::X as
/// core::ops::Fn>::call` -> `freshbench`.
fn root_crate(name: &str) -> &str {
    let mut s = name.trim_start_matches(['<', '&', '*', ' ']);
    for prefix in ["mut ", "const ", "dyn "] {
        s = s.strip_prefix(prefix).unwrap_or(s);
    }
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// A frame's function name when it is in a crate of [`FRAME_CRATES`] and
/// looks like a demangled Rust path; [`OTHER_FRAME`] otherwise.
fn clean_function(name: &str) -> String {
    let rust_path = name.len() <= 512
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" _:<>{}()[]&*,;'#!-.".contains(&b));
    if rust_path && FRAME_CRATES.contains(&root_crate(name)) {
        name.to_owned()
    } else {
        OTHER_FRAME.to_owned()
    }
}

fn scrub_frame(frame: Frame) -> Frame {
    let file = frame
        .abs_path
        .as_deref()
        .or(frame.filename.as_deref())
        .and_then(clean_source_path);
    Frame {
        function: frame.function.as_deref().map(clean_function),
        lineno: file.as_ref().and(frame.lineno),
        filename: file,
        in_app: frame.in_app,
        ..Default::default()
    }
}

fn scrub_stacktrace(stacktrace: Stacktrace) -> Stacktrace {
    Stacktrace {
        frames: stacktrace.frames.into_iter().map(scrub_frame).collect(),
        frames_omitted: stacktrace.frames_omitted,
        ..Default::default()
    }
}

fn scrub_exception(exception: Exception, location: Option<&str>) -> Exception {
    let ty = one_of(&exception.ty, EXCEPTION_TYPES)
        .unwrap_or("error")
        .to_owned();
    let value = match location {
        Some(l) => format!("{ty} at {l}"),
        None => format!("{ty} (message not sent)"),
    };
    Exception {
        value: Some(value),
        mechanism: exception.mechanism.map(|m| Mechanism {
            ty: one_of(&m.ty, MECHANISM_TYPES)
                .unwrap_or("generic")
                .to_owned(),
            handled: m.handled,
            synthetic: m.synthetic,
            ..Default::default()
        }),
        stacktrace: exception.stacktrace.map(scrub_stacktrace),
        ty,
        ..Default::default()
    }
}

/// Last line of defence before an event leaves the process. Allowlist: the
/// event is rebuilt from the fields named in the module documentation, each
/// checked against its own rule; nothing else is copied.
fn scrub(event: Event<'static>) -> Event<'static> {
    let location = event
        .tags
        .get(LOCATION_TAG)
        .filter(|l| is_clean_location(l))
        .cloned();
    let exception: Vec<Exception> = event
        .exception
        .values
        .into_iter()
        .map(|e| scrub_exception(e, location.as_deref()))
        .collect();
    let ty = exception
        .first()
        .map_or_else(|| "message".to_owned(), |e| e.ty.clone());
    let mut out = Event::new();
    out.event_id = event.event_id;
    out.level = event.level;
    out.timestamp = event.timestamp;
    out.fingerprint = Cow::Owned(vec![
        Cow::Owned(ty),
        Cow::Owned(location.clone().unwrap_or_else(|| NO_LOCATION.to_owned())),
    ]);
    out.platform = Cow::Borrowed(one_of(&event.platform, PLATFORMS).unwrap_or("native"));
    out.release = event
        .release
        .as_deref()
        .and_then(clean_release)
        .map(|r| Cow::Owned(r.to_owned()));
    out.environment = event
        .environment
        .as_deref()
        .and_then(clean_environment)
        .map(Cow::Borrowed);
    out.message = event.message.map(|_| REDACTED.to_owned());
    out.logentry = event.logentry.map(|_| LogEntry {
        message: REDACTED.to_owned(),
        params: Vec::new(),
    });
    if let Some(l) = location {
        out.tags.insert(LOCATION_TAG.into(), l);
    }
    out.sdk = event.sdk.map(|sdk| {
        Cow::Owned(ClientSdkInfo {
            name: one_of(&sdk.name, SDK_NAMES).unwrap_or("unknown").to_owned(),
            version: clean_version(&sdk.version).unwrap_or("unknown").to_owned(),
            integrations: Vec::new(),
            packages: Vec::new(),
        })
    });
    out.exception = Values::from(exception);
    out.stacktrace = event.stacktrace.map(scrub_stacktrace);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Breadcrumb, Context, Envelope, Request, Thread, User};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn json(event: &Event<'static>) -> String {
        serde_json::to_string(event).unwrap()
    }

    /// An event whose every free-text field holds `text`.
    fn event_with_text(text: &str) -> Event<'static> {
        let mut event = Event::new();
        event.message = Some(text.into());
        event.transaction = Some(text.into());
        event.logger = Some(text.into());
        event.culprit = Some(text.into());
        event.server_name = Some(text.to_owned().into());
        event.release = Some(text.to_owned().into());
        event.environment = Some(text.to_owned().into());
        event.dist = Some(text.to_owned().into());
        event.logentry = Some(LogEntry {
            message: text.into(),
            params: vec![text.into()],
        });
        event.modules.insert(text.into(), text.into());
        event.tags.insert("path".into(), text.into());
        event.tags.insert(LOCATION_TAG.into(), text.into());
        event.extra.insert("argv".into(), text.into());
        event.contexts.insert(
            "os".into(),
            Context::Other(BTreeMap::from([("name".to_owned(), text.into())])),
        );
        event.breadcrumbs.values.push(Breadcrumb {
            message: Some(text.into()),
            ..Default::default()
        });
        event.user = Some(User {
            username: Some(text.into()),
            ..Default::default()
        });
        event.request = Some(Request {
            data: Some(text.into()),
            ..Default::default()
        });
        let frame = || Frame {
            function: Some(text.into()),
            symbol: Some(text.into()),
            module: Some(text.into()),
            package: Some(text.into()),
            filename: Some(text.into()),
            abs_path: Some(text.into()),
            lineno: Some(7),
            context_line: Some(text.into()),
            pre_context: vec![text.into()],
            post_context: vec![text.into()],
            vars: BTreeMap::from([(text.to_owned(), text.into())]),
            ..Default::default()
        };
        event.threads.values.push(Thread {
            name: Some(text.into()),
            stacktrace: Some(Stacktrace {
                frames: vec![frame()],
                ..Default::default()
            }),
            ..Default::default()
        });
        event.exception.values.push(Exception {
            ty: text.into(),
            value: Some(text.into()),
            module: Some(text.into()),
            mechanism: Some(Mechanism {
                ty: text.into(),
                description: Some(text.into()),
                data: BTreeMap::from([(text.to_owned(), text.into())]),
                ..Default::default()
            }),
            stacktrace: Some(Stacktrace {
                frames: vec![frame()],
                ..Default::default()
            }),
            ..Default::default()
        });
        event.stacktrace = Some(Stacktrace {
            frames: vec![frame()],
            ..Default::default()
        });
        event
    }

    /// `needle` (or, when it is long, any 4-byte window of it) in `hay`.
    fn leaks<'a>(hay: &str, needle: &'a str) -> Option<&'a str> {
        let b = needle.as_bytes();
        if b.len() < 4 {
            return hay.contains(needle).then_some(needle);
        }
        (0..=b.len() - 4)
            .filter_map(|i| needle.get(i..i + 4))
            .find(|w| hay.contains(w))
    }

    #[test]
    fn off_without_explicit_opt_in() {
        assert_eq!(
            dsn_from(env(&[("SENTRY_DSN", "https://k@o1.ingest.sentry.io/2")])),
            None
        );
        assert_eq!(
            dsn_from(env(&[
                ("FRESHBENCH_SENTRY", "0"),
                ("SENTRY_DSN", "https://k@o1/2")
            ])),
            None
        );
        assert_eq!(dsn_from(env(&[("FRESHBENCH_SENTRY", "1")])), None);
        assert_eq!(dsn_from(env(&[])), None);
    }

    #[test]
    fn on_only_with_opt_in_and_dsn() {
        let dsn = "https://k@o1.ingest.sentry.io/2";
        let got = dsn_from(env(&[("FRESHBENCH_SENTRY", "1"), ("SENTRY_DSN", dsn)]));
        assert_eq!(got.as_deref(), Some(dsn));
        assert_eq!(
            dsn_from(env(&[("FRESHBENCH_SENTRY", "1"), ("SENTRY_DSN", "  ")])),
            None
        );
    }

    #[test]
    fn options_install_only_panic_and_stacktrace_integrations() {
        let opts = options("https://k@o1.ingest.sentry.io/2", env(&[]));
        assert!(!opts.default_integrations);
        assert!(!opts.send_default_pii);
        assert!(!opts.attach_stacktrace);
        assert_eq!(opts.max_breadcrumbs, 0);
        assert!(opts.before_send.is_some());
        let names: Vec<&str> = opts.integrations.iter().map(|i| i.name()).collect();
        assert_eq!(names, ["panic", "process-stacktrace"]);
        let opts = sentry::apply_defaults(opts);
        let names: Vec<&str> = opts.integrations.iter().map(|i| i.name()).collect();
        assert_eq!(names, ["panic", "process-stacktrace"]);
    }

    #[test]
    fn release_and_environment_are_clean_or_default() {
        assert_eq!(
            release(env(&[("SENTRY_RELEASE", " freshbench@2.0.0+abc ")])),
            "freshbench@2.0.0+abc"
        );
        assert_eq!(
            release(env(&[
                ("SENTRY_RELEASE", "token=ghp_x /home/alice"),
                ("FRESHBENCH_RELEASE", "freshbench@1")
            ])),
            "freshbench@1"
        );
        assert!(release(env(&[("SENTRY_RELEASE", "other@1")])).starts_with("freshbench@"));
        assert!(release(env(&[])).starts_with("freshbench@"));
        assert_eq!(environment(env(&[("SENTRY_ENVIRONMENT", "ci")])), "ci");
        assert_eq!(
            environment(env(&[("SENTRY_ENVIRONMENT", "/Users/alice x")])),
            "production"
        );
    }

    #[test]
    fn source_paths_keep_only_public_forms() {
        let cases = [
            ("crates/freshbench/src/score.rs", Some("crates/freshbench/src/score.rs")),
            ("crates\\freshbench\\src\\ged.rs", Some("crates/freshbench/src/ged.rs")),
            ("crates/rust-joern/src/cfg.rs", Some("crates/rust-joern/src/cfg.rs")),
            (
                "/home/alice/work/fb/crates/freshbench/src/main.rs",
                Some("crates/freshbench/src/main.rs"),
            ),
            (
                "C:\\Users\\alice\\fb\\crates\\freshbench\\src\\main.rs",
                Some("crates/freshbench/src/main.rs"),
            ),
            (
                "/rustc/0ef1bc386abc/library/core/src/panicking.rs",
                Some("library/core/src/panicking.rs"),
            ),
            (
                "/Users/alice/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rayon-core-1.13.0/src/job.rs",
                Some("deps/rayon-core-1.13.0/src/job.rs"),
            ),
            (
                "C:\\Users\\alice\\.cargo\\registry\\src\\index.crates.io-1\\gimli-0.33.0\\src\\read.rs",
                Some("deps/gimli-0.33.0/src/read.rs"),
            ),
            ("/Users/alice/My Projects/secret plan.rs", None),
            ("/Users/alice/My Projects/crates/freshbench/src/x y.rs", None),
            ("/Users/alice/.cargo/git/checkouts/private-dep/src/lib.rs", None),
            ("\\\\fileserver\\share\\Alice Docs\\x.rs", None),
            ("/data/corpus/m1/proj/src/main.c", None),
            ("crates/freshbench/../../../etc/passwd.rs", None),
            ("src/main.rs", None),
            ("", None),
        ];
        for (raw, want) in cases {
            assert_eq!(clean_source_path(raw).as_deref(), want, "{raw}");
        }
        assert!(is_clean_location("crates/freshbench/src/score.rs:12"));
        assert!(!is_clean_location("crates/freshbench/src/score.rs"));
        assert!(!is_clean_location("crates/freshbench/src/score.rs:1a"));
        assert!(!is_clean_location("/Users/alice/x.rs:1"));
    }

    #[test]
    fn frame_functions_keep_only_known_crates() {
        for name in [
            "freshbench::score::score",
            "<freshbench::ged::Ged as core::ops::function::Fn<(&str,)>>::call",
            "core::panicking::panic_fmt",
            "std::panicking::begin_panic_handler::{{closure}}",
            "rayon_core::registry::WorkerThread::wait_until_cold",
            "rust_joern::cfg::build",
        ] {
            assert_eq!(clean_function(name), name);
        }
        for name in [
            "tree_sitter::Parser::parse",
            "private_internal_crate::secret::fn",
            "<unknown>",
            "main",
            "freshbench::score::token=ghp_abcdefgh",
            "freshbench::score::/Users/alice/x",
        ] {
            assert_eq!(clean_function(name), OTHER_FRAME, "{name}");
        }
    }

    /// Each case is a string one Fission or FreshBench PR scrubber (word-split
    /// redaction) let through; none of its secret part may survive.
    #[test]
    fn free_text_never_leaves_the_process() {
        let cases = [
            ("token=ghp_abcdef1234567890 failed", "ghp_abcdef1234567890"),
            ("api_key=\"sk-live-QWERTY\"", "QWERTY"),
            (
                "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.c2VjcmV0",
                "eyJhbGciOiJIUzI1NiJ9",
            ),
            (
                "fetch https://alice:hunter2@example.com/x failed",
                "hunter2",
            ),
            (
                "bad dsn https://0123456789abcdef@o1.ingest.sentry.io/2",
                "0123456789abcdef",
            ),
            (
                "cannot read /Users/alice/My Projects/secret plan.bin",
                "secret plan",
            ),
            (
                "cannot read /Users/alice/My Projects/secret plan.bin",
                "My Projects",
            ),
            (
                "open \\\\fileserver\\share\\Alice Docs\\x.bin",
                "Alice Docs",
            ),
            ("open C:\\Users\\Bob Smith\\Desktop\\q.exe", "Bob Smith"),
            (
                "unexpected token `int secret_fn(char *passwd)` at byte 41",
                "secret_fn",
            ),
            (
                "output: if (uVar3 == 0x41414141) { return param_1; }",
                "uVar3",
            ),
        ];
        for (text, secret) in cases {
            let out = json(&scrub(event_with_text(text)));
            assert!(
                !out.contains(secret),
                "{secret:?} leaked from {text:?}: {out}"
            );
            assert!(leaks(&out, secret).is_none(), "{secret:?}: {out}");
            assert!(
                json(&event_with_text(text)).contains(secret),
                "vacuous: {text}"
            );
        }
    }

    #[test]
    fn scrub_keeps_only_the_allowlist() {
        let event = scrub(event_with_text("x"));
        assert!(event.breadcrumbs.values.is_empty());
        assert!(event.extra.is_empty());
        assert!(event.contexts.is_empty());
        assert!(event.modules.is_empty());
        assert!(event.threads.values.is_empty());
        assert!(event.server_name.is_none());
        assert!(event.user.is_none());
        assert!(event.request.is_none());
        assert!(event.transaction.is_none());
        assert!(event.logger.is_none());
        assert!(event.culprit.is_none());
        assert!(event.dist.is_none());
        assert_eq!(event.tags.len(), 0, "an unclean location tag is dropped");
        assert_eq!(event.message.as_deref(), Some(REDACTED));
        let log = event.logentry.as_ref().unwrap();
        assert_eq!(log.message, REDACTED);
        assert!(log.params.is_empty());
        let exception = &event.exception.values[0];
        assert!(exception.module.is_none());
        let mechanism = exception.mechanism.as_ref().unwrap();
        assert!(mechanism.description.is_none());
        assert!(mechanism.data.is_empty());
        for frame in exception
            .stacktrace
            .iter()
            .chain(event.stacktrace.iter())
            .flat_map(|s| &s.frames)
        {
            assert!(frame.module.is_none());
            assert!(frame.package.is_none());
            assert!(frame.abs_path.is_none());
            assert!(frame.symbol.is_none());
            assert!(frame.vars.is_empty());
            assert!(frame.pre_context.is_empty());
            assert!(frame.post_context.is_empty());
            assert!(frame.context_line.is_none());
            assert!(frame.instruction_addr.is_none());
            assert_eq!(frame.function.as_deref(), Some(OTHER_FRAME));
            assert!(frame.filename.is_none());
            assert!(frame.lineno.is_none(), "no line without a clean file");
        }
    }

    #[test]
    fn frames_keep_clean_function_file_and_line() {
        let frame = scrub_frame(Frame {
            function: Some("freshbench::score::score".into()),
            symbol: Some("_ZN10freshbench5score5score17h0123456789abcdefE".into()),
            module: Some("freshbench::score".into()),
            package: Some("/Users/alice/work/target/release/freshbench".into()),
            filename: Some("score.rs".into()),
            abs_path: Some("/Users/alice/work/fb/crates/freshbench/src/score.rs".into()),
            lineno: Some(42),
            colno: Some(9),
            in_app: Some(true),
            instruction_addr: Some(0x1000.into()),
            ..Default::default()
        });
        assert_eq!(frame.function.as_deref(), Some("freshbench::score::score"));
        assert_eq!(
            frame.filename.as_deref(),
            Some("crates/freshbench/src/score.rs")
        );
        assert_eq!(frame.lineno, Some(42));
        assert_eq!(frame.in_app, Some(true));
        assert!(frame.abs_path.is_none() && frame.module.is_none() && frame.package.is_none());
        assert!(frame.symbol.is_none() && frame.colno.is_none());
        assert!(frame.instruction_addr.is_none());
    }

    #[test]
    fn fingerprint_is_type_and_location() {
        let mut event = Event::new();
        event.tags.insert(
            LOCATION_TAG.into(),
            "crates/freshbench/src/score.rs:42".into(),
        );
        event.tags.insert("other".into(), "v".into());
        event.exception.values.push(Exception {
            ty: "panic".into(),
            value: Some("index out of bounds: the len is 3 but the index is 7".into()),
            ..Default::default()
        });
        let event = scrub(event);
        assert_eq!(
            event
                .fingerprint
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>(),
            ["panic", "crates/freshbench/src/score.rs:42"]
        );
        assert_eq!(
            event.tags.keys().map(String::as_str).collect::<Vec<_>>(),
            [LOCATION_TAG]
        );
        assert_eq!(
            event.exception.values[0].value.as_deref(),
            Some("panic at crates/freshbench/src/score.rs:42")
        );
        let bare = scrub(Event::new());
        assert_eq!(
            bare.fingerprint
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>(),
            ["message", NO_LOCATION]
        );
    }

    /// Deterministic generator: random "analyzed data" (uppercase tokens,
    /// which nothing FreshBench itself writes into an event contains four of
    /// in a row) is wrapped in the shapes real data takes and planted in every
    /// field; no 4-byte window of any token may reach the serialized event.
    ///
    /// Not generated: data shaped exactly like a FreshBench source path or a
    /// function path rooted in one of `FRAME_CRATES` (`std::X::Y`). A frame
    /// keeps those by design; its file and function come from the binary's
    /// own debug info, never from analyzed data.
    #[test]
    fn generated_data_never_reaches_the_event() {
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let shapes = [
            "{}",
            "/Users/{} {}/{}.bin",
            "C:\\Users\\{} {}\\{}.exe",
            "\\\\{}\\{} {}\\x",
            "token={} key: {} secret={}",
            "Authorization: Bearer {}.{}",
            "https://{}:{}@{}.example/x",
            "int {}(char *{}) {{ return {}; }}",
            "/Users/x/crates/freshbench/src/{} {}.rs",
            "{}::{}(x) at {}:{}",
            "{}:12",
        ];
        for round in 0..500 {
            let mut tokens = Vec::new();
            let mut token = || {
                let len = 8 + (next() % 17) as usize;
                let t: String = (0..len)
                    .map(|_| char::from(b'A' + (next() % 26) as u8))
                    .collect();
                tokens.push(t.clone());
                t
            };
            let shape = shapes[round % shapes.len()];
            let mut text = String::new();
            let mut rest = shape;
            while let Some(i) = rest.find("{}") {
                text.push_str(&rest[..i]);
                text.push_str(&token());
                rest = &rest[i + 2..];
            }
            text.push_str(rest);
            let text = text.replace("{{", "{").replace("}}", "}");
            let raw = json(&event_with_text(&text));
            let out = json(&scrub(event_with_text(&text)));
            for t in &tokens {
                assert!(leaks(&raw, t).is_some(), "vacuous: {t}");
                if let Some(w) = leaks(&out, t) {
                    panic!("round {round}: {w:?} of {t:?} from {text:?} leaked: {out}");
                }
            }
        }
    }

    #[derive(Default)]
    struct Captured(Mutex<Vec<Envelope>>);

    impl sentry::Transport for Captured {
        fn send_envelope(&self, envelope: Envelope) {
            self.0.lock().unwrap().push(envelope);
        }
    }

    /// A real panic through the real client and panic hook, read back as the
    /// bytes the transport would put on the wire. No network: the transport
    /// is in memory.
    #[test]
    fn a_real_panic_sends_only_type_location_and_frames() {
        let secret = "QXJWZKVLMNOPRSTUY";
        let captured = Arc::new(Captured::default());
        let opts =
            options("https://public@example.invalid/1", env(&[])).transport(captured.clone());
        let client = Arc::new(sentry::Client::with_options(sentry::apply_defaults(opts)));
        let hub = Arc::new(sentry::Hub::new(Some(client), Arc::default()));
        let line = sentry::Hub::run(hub, || {
            let line = line!() + 2;
            let r = std::panic::catch_unwind(|| {
                panic!("token=ghp_{secret} /Users/alice/My Projects/{secret}.bin")
            });
            assert!(r.is_err());
            line
        });
        let envelopes = captured.0.lock().unwrap();
        assert_eq!(envelopes.len(), 1);
        let mut wire = Vec::new();
        envelopes[0].to_writer(&mut wire).unwrap();
        let wire = String::from_utf8_lossy(&wire);
        assert!(leaks(&wire, secret).is_none(), "{wire}");
        for word in ["ghp_", "alice", "Projects", "token="] {
            assert!(!wire.contains(word), "{word}: {wire}");
        }
        let location = format!("crates/freshbench/src/telemetry.rs:{line}");
        let event = envelopes[0].event().unwrap();
        assert_eq!(event.tags.get(LOCATION_TAG), Some(&location));
        assert_eq!(event.fingerprint[0], "panic");
        assert_eq!(event.fingerprint[1], location);
        let exception = &event.exception.values[0];
        assert_eq!(exception.ty, "panic");
        assert_eq!(
            exception.value.as_deref(),
            Some(&*format!("panic at {location}"))
        );
        let frames = &exception.stacktrace.as_ref().unwrap().frames;
        assert!(frames.iter().any(|f| f
            .function
            .as_deref()
            .is_some_and(|f| f.starts_with("freshbench::telemetry::tests::"))));
        for f in frames {
            assert!(f.abs_path.is_none() && f.module.is_none() && f.package.is_none());
        }
        assert!(event.modules.is_empty() && event.contexts.is_empty());
    }
}
