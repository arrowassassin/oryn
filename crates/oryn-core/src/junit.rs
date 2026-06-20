//! Parse JUnit XML emitted by `cargo nextest run --profile <p>` into per-test
//! outcomes.
//!
//! nextest writes one `<testsuite>` per test binary and one `<testcase>` per
//! test, with a `time` attribute (seconds), a `<failure>`/`<error>` child on
//! failure, and `<flakyFailure>`/`<rerun>` children when a test was retried.
//! This is the stable, portable channel for collecting per-test pass/fail and
//! duration history.

use crate::{OrynError, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

/// One test's outcome from a JUnit report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestOutcome {
    /// Stable id: `<suite>::<testcase name>`.
    pub id: String,
    /// True if the test ultimately passed.
    pub passed: bool,
    /// True if the test was retried (passed only after a failure).
    pub flaky: bool,
    /// Wall-clock duration in milliseconds, if reported.
    pub duration_ms: Option<u64>,
}

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if a.key.as_ref() == key {
            Some(String::from_utf8_lossy(&a.value).into_owned())
        } else {
            None
        }
    })
}

/// Build the `(id, duration_ms, failed, flaky)` tuple for a `<testcase>` start.
fn testcase_id(
    e: &quick_xml::events::BytesStart,
    suite: &str,
) -> (String, Option<u64>, bool, bool) {
    let name = attr(e, b"name").unwrap_or_default();
    let id = if suite.is_empty() {
        name
    } else {
        format!("{suite}::{name}")
    };
    let dur = attr(e, b"time")
        .and_then(|t| t.parse::<f64>().ok())
        .map(|s| (s * 1000.0).round() as u64);
    (id, dur, false, false)
}

/// Parse JUnit XML bytes into per-test outcomes.
///
/// # Errors
/// Returns an error if the XML is malformed.
pub fn parse(xml: &[u8]) -> Result<Vec<TestOutcome>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut out = Vec::new();
    let mut suite = String::new();
    // Current testcase being assembled.
    let mut cur: Option<(String, Option<u64>, bool, bool)> = None; // (id, dur, failed, flaky)

    loop {
        match reader
            .read_event()
            .map_err(|e| OrynError::Process(format!("junit parse: {e}")))?
        {
            Event::Eof => break,
            Event::Start(e) => match e.name().as_ref() {
                b"testsuite" => {
                    if let Some(n) = attr(&e, b"name") {
                        suite = n;
                    }
                }
                b"testcase" => cur = Some(testcase_id(&e, &suite)),
                b"failure" | b"error" => {
                    if let Some(c) = cur.as_mut() {
                        c.2 = true;
                    }
                }
                b"flakyFailure" | b"flakyError" | b"rerun" | b"rerunFailure" => {
                    if let Some(c) = cur.as_mut() {
                        c.3 = true;
                    }
                }
                _ => {}
            },
            Event::Empty(e) => match e.name().as_ref() {
                // A self-closing <testcase/> passed with no children.
                b"testcase" => {
                    let (id, dur, _, _) = testcase_id(&e, &suite);
                    out.push(TestOutcome {
                        id,
                        passed: true,
                        flaky: false,
                        duration_ms: dur,
                    });
                }
                b"failure" | b"error" => {
                    if let Some(c) = cur.as_mut() {
                        c.2 = true;
                    }
                }
                b"flakyFailure" | b"flakyError" | b"rerun" | b"rerunFailure" => {
                    if let Some(c) = cur.as_mut() {
                        c.3 = true;
                    }
                }
                _ => {}
            },
            Event::End(e) => match e.name().as_ref() {
                b"testcase" => {
                    if let Some((id, dur, failed, flaky)) = cur.take() {
                        out.push(TestOutcome {
                            id,
                            passed: !failed,
                            flaky: flaky && !failed,
                            duration_ms: dur,
                        });
                    }
                }
                b"testsuite" => suite.clear(),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0"?>
<testsuites>
  <testsuite name="oryn-core" tests="3">
    <testcase name="graph::tests::ok" time="0.0012"/>
    <testcase name="select::tests::bad" time="0.05">
      <failure message="assertion failed">boom</failure>
    </testcase>
    <testcase name="net::flaky" time="0.2">
      <flakyFailure message="timeout"/>
    </testcase>
  </testsuite>
</testsuites>"#;

    #[test]
    fn parses_pass_fail_flaky_and_durations() {
        let outs = parse(SAMPLE).unwrap();
        assert_eq!(outs.len(), 3);

        let ok = &outs[0];
        assert_eq!(ok.id, "oryn-core::graph::tests::ok");
        assert!(ok.passed && !ok.flaky);
        assert_eq!(ok.duration_ms, Some(1)); // 0.0012s -> 1ms

        let bad = &outs[1];
        assert_eq!(bad.id, "oryn-core::select::tests::bad");
        assert!(!bad.passed);
        assert_eq!(bad.duration_ms, Some(50));

        let flaky = &outs[2];
        assert!(flaky.passed && flaky.flaky);
        assert_eq!(flaky.duration_ms, Some(200));
    }

    #[test]
    fn empty_report_is_empty() {
        let outs = parse(b"<testsuites></testsuites>").unwrap();
        assert!(outs.is_empty());
    }
}
