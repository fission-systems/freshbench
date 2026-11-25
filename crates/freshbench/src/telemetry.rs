//! Opt-in error reporting to Sentry, compiled only with the `sentry` feature.
//!
//! Nothing is sent unless `FRESHBENCH_SENTRY=1` and `SENTRY_DSN` are both set.
//! Only panics are reported here. Output text, binaries, decompiled code and
//! environment variables never leave the process, and file paths are reduced
//! to their base names before an event is sent.

use sentry::protocol::{Event, Exception, Frame};

/// Keeps the Sentry client alive; dropping it flushes queued events.
pub type Guard = sentry::ClientInitGuard;

/// The DSN to report to, or `None` when reporting is not enabled.
fn dsn_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    if lookup("FRESHBENCH_SENTRY").as_deref() != Some("1") {
        return None;
    }
    lookup("SENTRY_DSN")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Start reporting if the environment asks for it. Call once, from `main`.
pub fn init() -> Option<Guard> {
    let dsn = dsn_from(|key| std::env::var(key).ok())?;
    if dsn.parse::<sentry::types::Dsn>().is_err() {
        eprintln!("telemetry: SENTRY_DSN is not a valid DSN; reporting stays off");
        return None;
    }
    Some(sentry::init(
        sentry::ClientOptions::new()
            .dsn(&dsn)
            .maybe_release(sentry::release_name!())
            .send_default_pii(false)
            .sample_rate(1.0)
            .traces_sample_rate(0.0)
            .attach_stacktrace(true)
            .max_breadcrumbs(0)
            .before_send(|event| Some(scrub(event))),
    ))
}

/// Replace absolute paths and the home directory in free text.
fn redact_text(text: &str) -> String {
    let home = std::env::var("HOME").ok().filter(|h| h.len() > 1);
    text.split(' ')
        .map(|word| {
            let word = match &home {
                Some(h) => word.replace(h.as_str(), "~"),
                None => word.to_owned(),
            };
            match word.rsplit('/').next() {
                Some(base) if word.contains('/') && word.len() > 1 => format!("<path>/{base}"),
                _ => word,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

fn scrub_frame(frame: &mut Frame) {
    frame.abs_path = frame.abs_path.take().map(|p| base_name(&p));
    frame.filename = frame.filename.take().map(|p| base_name(&p));
    frame.vars.clear();
}

fn scrub_exception(exception: &mut Exception) {
    exception.value = exception.value.take().map(|v| redact_text(&v));
    if let Some(stacktrace) = exception.stacktrace.as_mut() {
        stacktrace.frames.iter_mut().for_each(scrub_frame);
    }
}

/// Last line of defence before an event leaves the process.
fn scrub(mut event: Event<'static>) -> Event<'static> {
    event.message = event.message.take().map(|m| redact_text(&m));
    event.breadcrumbs = Default::default();
    event.extra.clear();
    event.request = None;
    event.user = None;
    event.server_name = None;
    event.exception.values.iter_mut().for_each(scrub_exception);
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Breadcrumb, Stacktrace, Values};
    use std::collections::BTreeMap;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
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
    fn redacts_paths_in_free_text() {
        let out = redact_text("cannot read /data/corpus/m1/a.c and keep 1/2 ratio");
        assert_eq!(out, "cannot read <path>/a.c and keep <path>/2 ratio");
        assert!(!out.contains("/data/corpus"));
    }

    #[test]
    fn scrub_drops_context_and_keeps_base_names() {
        let mut event = Event::new();
        event.message = Some("failed in /home/alice/build/x".into());
        event.breadcrumbs = Values::new();
        event.breadcrumbs.values.push(Breadcrumb {
            message: Some("secret".into()),
            ..Default::default()
        });
        event.extra.insert("argv".into(), "--token abc".into());
        event.server_name = Some("alice-laptop".into());
        let mut frame = Frame {
            abs_path: Some("/home/alice/src/score.rs".into()),
            filename: Some("/home/alice/src/score.rs".into()),
            function: Some("score::run".into()),
            ..Default::default()
        };
        frame.vars = BTreeMap::from([("buf".to_owned(), "binary bytes".into())]);
        event.exception.values.push(Exception {
            value: Some("panic at /home/alice/src/score.rs".into()),
            stacktrace: Some(Stacktrace {
                frames: vec![frame],
                ..Default::default()
            }),
            ..Default::default()
        });

        let event = scrub(event);
        assert!(event.breadcrumbs.values.is_empty());
        assert!(event.extra.is_empty());
        assert!(event.server_name.is_none());
        assert!(!event.message.as_deref().unwrap_or("").contains("alice"));

        let exception = &event.exception.values[0];
        assert!(!exception.value.as_deref().unwrap_or("").contains("alice"));
        let frame = &exception.stacktrace.as_ref().unwrap().frames[0];
        assert_eq!(frame.abs_path.as_deref(), Some("score.rs"));
        assert_eq!(frame.filename.as_deref(), Some("score.rs"));
        assert_eq!(frame.function.as_deref(), Some("score::run"));
        assert!(frame.vars.is_empty());
    }
}
