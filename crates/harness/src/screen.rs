//! A session's screen before its harness is up: blank, or a shell's. Text
//! typed at a shell is a command it runs, so what is typed waits for the
//! harness's own screen.

/// The last row with anything on it.
pub(crate) fn last_row(screen: &[String]) -> Option<&str> {
    screen.iter().rev().map(|row| row.trim_end()).find(|row| !row.trim().is_empty())
}

/// Whether `row` is a shell's prompt, with or without a line typed after
/// it, as zsh and bash draw it by default: `me@host dir % `, `me@host:dir$ `,
/// `host:dir me$ `, `bash-3.2$ `.
pub(crate) fn shell_prompt(row: &str) -> bool {
    let words: Vec<&str> = row.split(' ').filter(|w| !w.is_empty()).collect();
    let ends = |w: &&str| w.ends_with('$') || w.ends_with('#');
    let user_host = |w: &&str| w.find('@').is_some_and(|i| i > 0 && i + 1 < w.len());
    match words.as_slice() {
        [first, _, mark, ..] if user_host(first) && matches!(*mark, "%" | "#") => true,
        [first, ..] if user_host(first) && first.contains(':') && ends(first) => true,
        [first, second, ..] if first.contains(':') && ends(second) && !second.contains(':') => true,
        [first, ..] => ["bash-", "sh-", "zsh-"].iter().any(|s| first.starts_with(s)) && ends(first),
        [] => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_shell_prompts_are_shells() {
        for row in [
            "felix@macnoMac-Studio control %",
            "felix@macnoMac-Studio web-1-1 % \u{1b}[200~Your task brief is the file /b.md\u{1b}[201~",
            "felix@macnoMac-Studio ~ % cd /w && export A=1 && kimi",
            "root@box ~ #",
            "me@host:~/web$ ls",
            "Mac:web me$",
            "bash-3.2$ ",
        ] {
            assert!(shell_prompt(row), "{row}");
        }
    }

    #[test]
    fn a_harness_s_rows_are_not_shells() {
        for row in [
            "? for shortcuts",
            "Context left until auto-compact: 12%",
            "› Write tests for @filename",
            "  100% context left · ? for shortcuts",
            "│ >                                           │",
            "Welcome to Kimi Code! Send /help for help information.",
            "felix@host wrote: 50 % done",
        ] {
            assert!(!shell_prompt(row), "{row}");
        }
    }

    #[test]
    fn the_last_row_skips_blank_ones() {
        let screen = ["a".to_string(), "b  ".into(), "   ".into(), String::new()];
        assert_eq!(last_row(&screen), Some("b"));
        assert_eq!(last_row(&[String::new()]), None);
    }
}
