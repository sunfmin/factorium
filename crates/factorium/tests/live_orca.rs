//! A tell against the real Orca and Claude Code on this host, run by hand:
//! the two checks `watch tell` makes, on a session busy in a call that never
//! returns (`cat` of a FIFO nobody writes). Its terminal handle goes in
//! `FACTORIUM_LIVE_TERMINAL`, its working directory in `FACTORIUM_LIVE_CWD`.
//!
//! `cargo test -p factorium --test live_orca -- --ignored --nocapture`

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use factorium::host::{Delivery, Orca, System};
use factorium_harness::Harness;

fn epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

#[test]
#[ignore = "needs a real Orca terminal, Claude Code in it busy in a hung call"]
fn a_busy_claude_code_queues_a_tell_and_reads_one_sent_now() {
    let (terminal, cwd) =
        (std::env::var("FACTORIUM_LIVE_TERMINAL").unwrap(), std::env::var("FACTORIUM_LIVE_CWD").unwrap());
    let home = std::env::var("HOME").unwrap();
    let h = Harness::ClaudeCode;
    let home = h.home(home.as_ref(), None, "/nonexistent".as_ref());
    let orca = Orca(&System);
    let nonce = epoch();
    let tell = |text: &str, key: Option<&str>| {
        let (start, since) = (Instant::now(), epoch() - 1);
        let seen = orca.deliver(&terminal, text, key).unwrap();
        let took = h.prompted(&home, &cwd, "", text, since);
        println!("{text:?}: orca {seen:?}, session took it: {took:?}, after {:.1}s", start.elapsed().as_secs_f64());
        (seen, took)
    };

    let (seen, took) = tell(&format!("Plain tell {nonce}: reply QUEUED-SEEN."), None);
    assert_eq!((seen, took), (Delivery::Queued, Some(false)), "a busy session only queues it");

    let text = format!("Told now {nonce}: reply NOW-SEEN and nothing else.");
    // Compare with the stamp of the `user` line its session wrote for the text.
    let sent_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    println!("--now sent at epoch ms {sent_ms}");
    let (seen, took) = tell(&text, Some(h.interrupt()));
    assert!(seen == Delivery::Submitted || took == Some(true), "it reads a tell sent now");
}
