/// Result of comparing the chosen image file to `show version` after the reboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareCheck {
    Pass,
    VersionMismatch,
    NoVersionToken,
}

/// The version in an image file name: the first three consecutive
/// dot-separated segments where the first two are all digits and the third is
/// digits optionally followed by lowercase letters (`17.09.04a`).
pub fn version_token(filename: &str) -> Option<String> {
    let segments: Vec<&str> = filename.split('.').collect();
    segments
        .windows(3)
        .find(|w| is_digits(w[0]) && is_digits(w[1]) && is_patch(w[2]))
        .map(|w| w.join("."))
}

/// `Pass` when `show_version` contains the image's version as a whole token.
pub fn firmware_check(filename: &str, show_version: &str) -> FirmwareCheck {
    let Some(token) = version_token(filename) else {
        return FirmwareCheck::NoVersionToken;
    };
    let bytes = show_version.as_bytes();
    let found = show_version
        .match_indices(&token)
        .any(|(start, _)| is_bounded(bytes, start, start + token.len()));
    if found {
        FirmwareCheck::Pass
    } else {
        FirmwareCheck::VersionMismatch
    }
}

fn is_digits(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit())
}

/// Digits, then optional lowercase letters.
fn is_patch(segment: &str) -> bool {
    let letters = segment.trim_start_matches(|c: char| c.is_ascii_digit());
    letters.len() < segment.len() && letters.bytes().all(|b| b.is_ascii_lowercase())
}

/// True when `bytes[start..end]` is not extended by a digit, or by a dot and a
/// digit, directly before `start` or directly after `end`.
fn is_bounded(bytes: &[u8], start: usize, end: usize) -> bool {
    let digit_at = |index: Option<usize>| {
        index
            .and_then(|i| bytes.get(i))
            .is_some_and(|b| b.is_ascii_digit())
    };
    let dot_at = |index: Option<usize>| index.and_then(|i| bytes.get(i)) == Some(&b'.');

    let glued_before = digit_at(start.checked_sub(1))
        || (dot_at(start.checked_sub(1)) && digit_at(start.checked_sub(2)));
    let glued_after = digit_at(Some(end)) || (dot_at(Some(end)) && digit_at(Some(end + 1)));
    !glued_before && !glued_after
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_filename_version_must_appear_in_show_version() {
        assert_eq!(
            firmware_check(
                "cat9k_iosxe.17.12.05.SPA.bin",
                "Cisco IOS XE Software, Version 17.12.05"
            ),
            FirmwareCheck::Pass
        );
        assert_eq!(
            firmware_check("cat9k_iosxe.17.12.05.SPA.bin", "Version 17.12.04"),
            FirmwareCheck::VersionMismatch
        );
        assert_eq!(
            firmware_check("image.bin", "Version 17.12.05"),
            FirmwareCheck::NoVersionToken
        );
    }

    #[test]
    fn token_is_extracted_from_the_image_filename() {
        assert_eq!(
            version_token("cat9k_iosxe.17.12.05.SPA.bin"),
            Some("17.12.05".to_string())
        );
        assert_eq!(version_token("image.bin"), None);
        assert_eq!(version_token("image.17.12.bin"), None);
    }

    #[test]
    fn token_is_read_from_dot_separated_filename_segments() {
        assert_eq!(
            version_token("isr4300-universalk9.16.09.04.SPA.bin"),
            Some("16.09.04".to_string())
        );
        assert_eq!(
            version_token("isr4300-universalk9.17.09.04a.SPA.bin"),
            Some("17.09.04a".to_string())
        );
        assert_eq!(
            version_token("c9300_image.16.9.4.SPA.bin"),
            Some("16.9.4".to_string())
        );
    }

    #[test]
    fn token_requires_digit_segments_and_a_lowercase_suffix_only() {
        assert_eq!(version_token("image.16.9.SPA.bin"), None);
        assert_eq!(version_token("image.16.9.A4.bin"), None);
        assert_eq!(version_token("image.16.9.4A.bin"), None);
        assert_eq!(version_token("image_16.9.4.bin"), None);
        assert_eq!(
            version_token("a.1.2.x.3.4.5b.bin"),
            Some("3.4.5b".to_string())
        );
    }

    #[test]
    fn suffixed_token_must_appear_whole_in_show_version() {
        let name = "isr4300-universalk9.17.09.04a.SPA.bin";
        assert_eq!(
            firmware_check(name, "Version 17.09.04a, RELEASE SOFTWARE"),
            FirmwareCheck::Pass
        );
        assert_eq!(
            firmware_check(name, "Version 17.09.04"),
            FirmwareCheck::VersionMismatch
        );
    }

    #[test]
    fn longer_version_in_show_version_is_not_a_match() {
        let name = "cat9k_iosxe.17.12.05.SPA.bin";
        assert_eq!(
            firmware_check(name, "Version 17.12.051"),
            FirmwareCheck::VersionMismatch
        );
        assert_eq!(
            firmware_check(name, "Version 117.12.05"),
            FirmwareCheck::VersionMismatch
        );
        assert_eq!(
            firmware_check(name, "Version 17.12.05.1"),
            FirmwareCheck::VersionMismatch
        );
        assert_eq!(
            firmware_check(name, "Version 3.17.12.05"),
            FirmwareCheck::VersionMismatch
        );
    }

    #[test]
    fn token_followed_by_a_non_digit_dot_is_a_match() {
        let name = "cat9k_iosxe.17.12.05.SPA.bin";
        assert_eq!(
            firmware_check(name, "Version 17.12.05, RELEASE SOFTWARE"),
            FirmwareCheck::Pass
        );
        assert_eq!(
            firmware_check(name, "Version 17.12.05."),
            FirmwareCheck::Pass
        );
    }

    #[test]
    fn a_later_bounded_occurrence_still_passes() {
        assert_eq!(
            firmware_check(
                "cat9k_iosxe.17.12.05.SPA.bin",
                "boot 117.12.05 then Version 17.12.05"
            ),
            FirmwareCheck::Pass
        );
    }

    #[test]
    fn no_token_takes_precedence_over_any_show_version() {
        assert_eq!(
            firmware_check("image.bin", ""),
            FirmwareCheck::NoVersionToken
        );
    }
}
