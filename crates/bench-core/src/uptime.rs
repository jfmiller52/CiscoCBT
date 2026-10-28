const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const MINUTES_PER_DAY: u64 = 24 * MINUTES_PER_HOUR;
const MINUTES_PER_WEEK: u64 = 7 * MINUTES_PER_DAY;
const MINUTES_PER_YEAR: u64 = 365 * MINUTES_PER_DAY;

/// One device's uptime, as reported by `show version | include uptime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uptime {
    pub minutes_total: u64,
}

/// Parses every `... uptime is <list>` line; other lines are ignored.
/// An `uptime is` line that does not parse fails the read: the result is
/// empty, so the burn-in cannot pass.
pub fn parse_uptime_lines(output: &str) -> Vec<Uptime> {
    let mut parsed = Vec::new();
    for line in output.lines() {
        let Some((_, list)) = line.split_once("uptime is") else {
            continue;
        };
        let Some(uptime) = parse_duration_list(list) else {
            return Vec::new();
        };
        parsed.push(uptime);
    }
    parsed
}

/// True when at least one line was parsed and every line meets `entered_hours`.
pub fn burn_in_passes(entered_hours: u32, lines: &[Uptime]) -> bool {
    let required = u64::from(entered_hours) * MINUTES_PER_HOUR;
    !lines.is_empty() && lines.iter().all(|line| line.minutes_total >= required)
}

/// Sums a comma-separated list like `1 year, 4 hours, 1 minute`.
/// Seconds count toward whole minutes. Returns `None` when any part is not a
/// number and a known unit.
fn parse_duration_list(list: &str) -> Option<Uptime> {
    let mut minutes: Option<u64> = None;
    let mut seconds: u64 = 0;
    let mut saw_part = false;
    for part in list.split(',') {
        let mut words = part.split_whitespace();
        let Some(number) = words.next() else {
            continue;
        };
        saw_part = true;
        let Some(unit) = words.next() else {
            return None;
        };
        if !number.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let count = number.parse::<u64>().unwrap_or(u64::MAX);
        if is_seconds(unit) {
            seconds = seconds.saturating_add(count);
            continue;
        }
        let Some(minutes_per_unit) = minutes_per_unit(unit) else {
            return None;
        };
        let added = count.saturating_mul(minutes_per_unit);
        minutes = Some(minutes.unwrap_or(0).saturating_add(added));
    }
    if !saw_part {
        return None;
    }
    let minutes_total = minutes
        .unwrap_or(0)
        .saturating_add(seconds / SECONDS_PER_MINUTE);
    Some(Uptime { minutes_total })
}

fn is_seconds(unit: &str) -> bool {
    matches!(unit, "second" | "seconds")
}

fn minutes_per_unit(unit: &str) -> Option<u64> {
    match unit {
        "year" | "years" => Some(MINUTES_PER_YEAR),
        "week" | "weeks" => Some(MINUTES_PER_WEEK),
        "day" | "days" => Some(MINUTES_PER_DAY),
        "hour" | "hours" => Some(MINUTES_PER_HOUR),
        "minute" | "minutes" => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_hours_six_minutes_passes_a_four_hour_entry() {
        let lines = parse_uptime_lines("closet-2-sw1 uptime is 4 hours, 6 minutes");
        assert!(burn_in_passes(4, &lines));
    }

    #[test]
    fn three_hours_fifty_eight_minutes_fails() {
        let lines = parse_uptime_lines("Switch uptime is 3 hours, 58 minutes");
        assert!(!burn_in_passes(4, &lines));
    }

    #[test]
    fn one_short_line_fails_the_burn_in() {
        let lines = parse_uptime_lines(
            "Switch uptime is 1 day, 2 hours\nSwitch uptime is 3 hours, 1 minute",
        );
        assert!(!burn_in_passes(4, &lines));
    }

    #[test]
    fn parses_years_weeks_days_hours_minutes() {
        let lines =
            parse_uptime_lines("Switch uptime is 1 year, 2 weeks, 3 days, 4 hours, 5 minutes");
        let expected = (365 + 14 + 3) * 24 * 60 + 4 * 60 + 5;
        assert_eq!(
            lines,
            vec![Uptime {
                minutes_total: expected
            }]
        );
    }

    #[test]
    fn parses_singular_single_unit() {
        let lines = parse_uptime_lines("R1 uptime is 1 minute");
        assert_eq!(lines, vec![Uptime { minutes_total: 1 }]);
    }

    #[test]
    fn ignores_lines_without_uptime_is() {
        let lines = parse_uptime_lines("Cisco IOS XE Software\nSwitch up for 4 hours");
        assert!(lines.is_empty());
    }

    #[test]
    fn an_unparsed_uptime_line_fails_the_burn_in() {
        let lines =
            parse_uptime_lines("Switch uptime is 4 hours, 6 minutes\nSwitch uptime is unknown");
        assert!(!burn_in_passes(4, &lines));
    }

    #[test]
    fn seconds_count_and_a_short_stack_member_fails() {
        let lines = parse_uptime_lines(
            "closet-2-sw1 uptime is 4 hours, 6 minutes\nSwitch uptime is 45 seconds",
        );
        assert_eq!(
            lines,
            vec![
                Uptime {
                    minutes_total: 4 * 60 + 6
                },
                Uptime { minutes_total: 0 }
            ]
        );
        assert!(!burn_in_passes(4, &lines));
        assert_eq!(
            parse_uptime_lines("R1 uptime is 90 seconds"),
            vec![Uptime { minutes_total: 1 }]
        );
        assert_eq!(
            parse_uptime_lines("R1 uptime is 1 second"),
            vec![Uptime { minutes_total: 0 }]
        );
    }

    #[test]
    fn ignores_uptime_line_without_recognizable_unit() {
        let lines = parse_uptime_lines("Switch uptime is unknown\nSwitch uptime is 5 fortnights");
        assert!(lines.is_empty());
    }

    #[test]
    fn empty_lines_fail_the_burn_in() {
        assert!(!burn_in_passes(4, &[]));
    }

    #[test]
    fn huge_numbers_saturate_instead_of_overflowing() {
        let lines = parse_uptime_lines(
            "Switch uptime is 99999999999999999999999 years, 18446744073709551615 weeks",
        );
        assert_eq!(
            lines,
            vec![Uptime {
                minutes_total: u64::MAX
            }]
        );
        assert!(burn_in_passes(u32::MAX, &lines));
    }
}
