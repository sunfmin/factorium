//! What a session's screen says stopped it, as each harness reads its own
//! screen's words.

use factorium_harness::{Harness, Stop};

fn screen(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_string()).collect()
}

/// Kimi Code's 5-hour limit is a kimi session's quota, kept as the line that
/// says so; wrapped at a word's end, which no one line holds, as the
/// limit's own words. Without its provider prefix, or on another
/// harness's screen, it is not.
#[test]
fn kimi_s_five_hour_usage_limit_is_its_quota_wrapped_or_not() {
    let kimi = Harness::read("kimi").unwrap();
    let line = "[provider.api_error] 429 You've reached your 5-hour usage limit. Resets at 21:00.";
    let quota = |s: &str| Some(Stop::Quota(s.into()));
    assert_eq!(kimi.stopped(&screen(&["> fix it", line, "~/w  k3"])), quota(line), "on one line");
    let cut = line.find("usage").unwrap();
    let limit = "[provider.api_error] 429 You've reached your 5-hour usage limit";
    assert_eq!(kimi.stopped(&screen(&[&line[..cut], &line[cut..]])), quota(limit), "wrapped at a word's end");
    assert_eq!(kimi.stopped(&screen(&["You've reached your 5-hour usage limit"])), None, "no provider prefix");
    assert_eq!(Harness::read("pi").unwrap().stopped(&screen(&[line])), None, "not pi's quota");
}
