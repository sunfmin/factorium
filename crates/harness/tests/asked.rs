//! What a session asked headless answered, as each harness prints it.

use std::path::Path;

use factorium_harness::{Ask, Asked, Harness};

/// What `agy --output-format json` printed on this host (agy 1.5.14), asked
/// "What is 2+3?" with its prompt on stdin.
const AGY_ANSWERED: &str = r#"{"conversation_id":"589a4476-c4ba-43cf-aa44-051d0d1edb9b","status":"SUCCESS","response":"5\n","duration_seconds":3.352896,"num_turns":1,"usage":{"input_tokens":2049,"output_tokens":27,"thinking_tokens":25,"cache_read_tokens":7841,"total_tokens":2076}}"#;
/// And given an empty prompt.
const AGY_FAILED: &str = r#"{"conversation_id":"","status":"ERROR","response":"","error":"Error: empty prompt. Usage: agy --print \"your prompt here\"","duration_seconds":0.000031375,"num_turns":0,"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},"finished_reason":"error"}"#;

/// agy's answer is its `response`, its session the conversation, its tokens
/// in its input and cache reads; it names no model, so the kit's is it.
#[test]
fn agy_s_real_answer_reads_as_its_conversation_text_and_tokens_cached_ones_among_them() {
    let agy = Harness::read("agy").unwrap();
    assert!(agy.asks());
    let asked = agy.asked(AGY_ANSWERED, Some("gemini-4-neon-high")).unwrap();
    let expected = Asked {
        session: "589a4476-c4ba-43cf-aa44-051d0d1edb9b".into(),
        model: "gemini-4-neon-high".into(),
        text: "5\n".into(),
        tokens_in: 2049 + 7841,
        tokens_out: 27,
    };
    assert_eq!(asked, expected);
    assert_eq!(agy.asked(AGY_ANSWERED, None).unwrap().model, "default");
}

/// A failed run, an empty answer, or no JSON is why it did not answer.
#[test]
fn agy_failing_or_answering_nothing_is_an_error() {
    let agy = Harness::read("agy").unwrap();
    let failed = agy.asked(AGY_FAILED, None).unwrap_err();
    assert!(failed.contains("empty prompt"), "{failed}");
    let blank = AGY_ANSWERED.replace(r#""response":"5\n""#, r#""response":"  ""#);
    assert!(agy.asked(&blank, None).unwrap_err().contains("answered nothing"));
    assert!(agy.asked("", None).unwrap_err().contains("no JSON"));
}

/// What each harness's stand-in prints reads back as what it was to answer.
#[test]
fn each_asking_harness_s_stand_in_reads_back_as_its_answer() {
    let asking: Vec<Harness> = Harness::all().iter().copied().filter(|h| h.asks()).collect();
    assert!(asking.contains(&Harness::Agy), "{asking:?}");
    for harness in asking {
        let answer = Asked { session: "t-1".into(), model: "m".into(), text: "ok".into(), tokens_in: 9, tokens_out: 4 };
        let printed = harness.asked_stand_in(&answer).unwrap();
        assert_eq!(harness.asked(&printed, Some("m")).unwrap(), answer, "{}", harness.key());
    }
}

/// agy is asked on stdin, no `-p` (it would take the next word as the
/// prompt), as the agent with no tools, with HOME its fresh user directory.
#[test]
fn agy_is_asked_on_stdin_as_its_toolless_agent_in_its_own_home() {
    let ask = Ask {
        command: "agy",
        model: Some("gemini-4-neon-high"),
        home: Path::new("/Users/u"),
        path: "/usr/bin",
        user_dir: Path::new("/tmp/pick/home"),
        question: "/tmp/pick/question.md",
        answer: "/tmp/pick/answer.json",
        secrets: None,
    };
    let line = Harness::Agy.ask_line(&ask).unwrap();
    assert!(line.starts_with("exec env -i HOME='/Users/u' PATH='/usr/bin' HOME='/tmp/pick/home' sh -c "), "{line}");
    for part in ["--output-format json", "--disable-slash-commands", "--agent factorium-ask", "command(*)", "--model"] {
        assert!(line.contains(part), "{part}: {line}");
    }
    let (_, redirected) = line.split_once(" < ").unwrap();
    assert!(redirected.contains("/tmp/pick/question.md") && redirected.contains("/tmp/pick/answer.json"), "{line}");
    assert!(!line.contains(" -p "), "{line}");
}
