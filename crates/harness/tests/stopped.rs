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

/// agy's quota is its engine's limit, reset as its screen says, its words
/// with the error's id; wrapped, the reset is read on the next row. With no
/// reset given, its account is used up. On another harness's screen it is
/// not.
#[test]
fn agy_s_quota_is_its_engine_s_limit_with_its_reset() {
    let agy = Harness::read("agy").unwrap();
    let line =
        "⚠ Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 2h50m4s.";
    let id = "Error ID: baec577a-1";
    let limit = |said: &str, ms: i64| Some(Stop::Limit { said: said.into(), resets_in_ms: ms, window: None });
    let at = (2 * 3600 + 50 * 60 + 4) * 1000;
    assert_eq!(agy.stopped(&screen(&["> go on", line, id, ""])), limit(&format!("{line} {id}"), at), "with its id");
    let cut = line.find("Resets").unwrap();
    let wrapped = screen(&[&line[..cut], &line[cut..]]);
    assert_eq!(agy.stopped(&wrapped), limit(line[..cut].trim(), at), "wrapped");
    let untold = "⚠ Individual quota reached. Please upgrade your subscription.";
    assert_eq!(agy.stopped(&screen(&[untold])), Some(Stop::Quota(untold.into())), "no reset given");
    let days = "⚠ Individual quota reached. Resets in 1d3h.";
    assert_eq!(agy.stopped(&screen(&[days])), limit(days, 27 * 3_600_000), "in days");
    assert_eq!(agy.stopped(&screen(&["> fix it", "~/w"])), None);
    assert_eq!(Harness::read("pi").unwrap().stopped(&screen(&[line])), None, "not pi's");
}

/// agy 1.5.17's quota of its Claude-and-GPT group ("Third-party", seen on
/// a horizon-labs worker 2026-11-26) is a limit too, reset as its screen
/// says, wrapped at the terminal's width as agy 1.5.14's: the group's 5-hour
/// window (`3p-5h`) for a reset within five hours, its weekly one past it.
#[test]
fn agy_s_third_party_quota_is_its_claude_and_gpt_window_s_limit() {
    let agy = Harness::read("agy").unwrap();
    let wrapped = screen(&[
        "⚠ Third-party quota reached. Please switch to Gemini models or upgrade your subscription to increase",
        "your limits.",
        "Resets in 2h17m29s.",
        "Error ID: 1ccc8031-f3ad-4089-b571-ae8837833b67-61",
        "────────────────────────────────────────",
        ">",
    ]);
    let said = "⚠ Third-party quota reached. Please switch to Gemini models or upgrade your subscription to increase \
                your limits. Resets in 2h17m29s. Error ID: 1ccc8031-f3ad-4089-b571-ae8837833b67-61";
    let at = (2 * 3600 + 17 * 60 + 29) * 1000;
    let limit = |said: &str, ms: i64, window| Some(Stop::Limit { said: said.into(), resets_in_ms: ms, window });
    assert_eq!(agy.stopped(&wrapped), limit(said, at, Some("3p-5h")), "wrapped, within five hours");
    let weekly = "⚠ Third-party quota reached. Resets in 3d4h.";
    assert_eq!(agy.stopped(&screen(&[weekly])), limit(weekly, 76 * 3_600_000, Some("3p-weekly")), "past five hours");
    let individual = screen(&[
        "⚠ Individual quota reached. Please upgrade your subscription to increase your",
        "limits. Resets in 2h50m4s.",
        "Error ID: baec577a-f31f-4b6e-8719-29962abbe30f-121",
    ]);
    let said = "⚠ Individual quota reached. Please upgrade your subscription to increase your \
                limits. Resets in 2h50m4s. Error ID: baec577a-f31f-4b6e-8719-29962abbe30f-121";
    assert_eq!(agy.stopped(&individual), limit(said, (2 * 3600 + 50 * 60 + 4) * 1000, None), "1.5.14's, as before");
}
