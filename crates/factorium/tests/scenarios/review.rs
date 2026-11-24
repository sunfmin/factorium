//! A growth reviewer's answer, by the domain's rule alone: one verdict of
//! one line, or none (factorium#304: once a never-built unit test of
//! `review.rs`).

use factorium_domain::review::{Reviewed, read};

#[test]
fn a_reviewer_s_answer_is_taken_only_as_one_verdict_of_one_line() {
    assert_eq!(read("```json\n{\"as_is\": \" new code only \"}\n```"), Ok(Reviewed::AsIs("new code only".into())));
    assert_eq!(read(r#"{"cut": "a.rs: reuse b"}"#), Ok(Reviewed::Cut("a.rs: reuse b".into())));
    for off in [r#"{"as_is": "x", "cut": "y"}"#, r#"{"as_is": "two\nlines"}"#, r#"{"cut": " "}"#, "{}", "as-is", "} {"]
    {
        assert!(read(off).is_err(), "{off}");
    }
}
