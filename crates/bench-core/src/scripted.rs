//! A console that plays back a fixed device conversation, so the session and
//! the run can be tested without a device.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use regex::Regex;

use crate::session::Console;

/// Plays back a device conversation. Each step is the text the session must
/// write, without its trailing `\r`, and the device output that becomes
/// readable after that write.
#[derive(Debug)]
pub struct ScriptedConsole {
    steps: VecDeque<(String, String)>,
    unread: String,
    written: Vec<String>,
}

impl ScriptedConsole {
    pub fn new(steps: Vec<(&str, &str)>) -> Self {
        Self {
            steps: steps
                .into_iter()
                .map(|(write, output)| (write.to_string(), output.to_string()))
                .collect(),
            unread: String::new(),
            written: Vec::new(),
        }
    }

    /// Every text the session wrote, without its trailing `\r`, including
    /// writes the script refused.
    pub fn written(&self) -> &[String] {
        &self.written
    }
}

impl Console for ScriptedConsole {
    fn write(&mut self, text: &str) -> io::Result<()> {
        let text = text.strip_suffix('\r').unwrap_or(text);
        self.written.push(text.to_string());
        let Some((expected, output)) = self.steps.pop_front() else {
            return Err(io::Error::other(format!(
                "the script has ended but the session wrote {text:?}"
            )));
        };
        if expected != text {
            let message =
                format!("the script expected {expected:?} but the session wrote {text:?}");
            self.steps.push_front((expected, output));
            return Err(io::Error::other(message));
        }
        self.unread.push_str(&output);
        Ok(())
    }

    /// Ignores `timeout`: when nothing unread matches, it fails at once.
    fn read_until(
        &mut self,
        patterns: &[Regex],
        _timeout: Duration,
    ) -> io::Result<(usize, String)> {
        let first = patterns
            .iter()
            .enumerate()
            .filter_map(|(index, pattern)| pattern.find(&self.unread).map(|m| (m.end(), index)))
            .min();
        let Some((end, index)) = first else {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("nothing matched the unread output {:?}", self.unread),
            ));
        };
        Ok((index, self.unread.drain(..end).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(sources: &[&str]) -> Vec<Regex> {
        sources.iter().map(|s| Regex::new(s).unwrap()).collect()
    }

    #[test]
    fn output_is_readable_only_after_the_matching_write() {
        let mut console = ScriptedConsole::new(vec![("show clock", "10:00\r\nSwitch#")]);
        let prompt = patterns(&["Switch#"]);
        let early = console.read_until(&prompt, Duration::ZERO).unwrap_err();
        assert_eq!(early.kind(), io::ErrorKind::TimedOut);
        console.write("show clock\r").unwrap();
        assert_eq!(
            console.read_until(&prompt, Duration::ZERO).unwrap(),
            (0, "10:00\r\nSwitch#".to_string())
        );
        assert_eq!(console.written(), &["show clock".to_string()]);
    }

    #[test]
    fn the_match_that_ends_first_wins_and_ties_go_to_the_lower_index() {
        let mut console = ScriptedConsole::new(vec![("", "xab")]);
        console.write("\r").unwrap();
        let set = patterns(&["b", "ab", "x"]);
        assert_eq!(
            console.read_until(&set, Duration::ZERO).unwrap(),
            (2, "x".to_string())
        );
        assert_eq!(
            console.read_until(&set, Duration::ZERO).unwrap(),
            (0, "ab".to_string())
        );
    }

    #[test]
    fn a_write_off_script_names_both_texts_and_keeps_the_step() {
        let mut console = ScriptedConsole::new(vec![("enable", "")]);
        let message = console.write("reload\r").unwrap_err().to_string();
        assert!(message.contains("\"enable\"") && message.contains("\"reload\""));
        console.write("enable\r").unwrap();
        assert_eq!(
            console.written(),
            &["reload".to_string(), "enable".to_string()]
        );
    }
}
