/// Result of comparing the chosen image file to `show version` after the reboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareCheck {
    Pass,
    VersionMismatch,
    NoVersionToken,
}

/// The first `digits.digits.digits` run in `filename` that is not glued to
/// another digit, or to a dot and a digit, on either side.
pub fn version_token(filename: &str) -> Option<String> {
    let bytes = filename.as_bytes();
    (0..bytes.len())
        .filter(|&start| bytes[start].is_ascii_digit())
        .find_map(|start| {
            let end = version_end(bytes, start)?;
            is_bounded(bytes, start, end).then(|| filename[start..end].to_string())
        })
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

/// End of the `digits.digits.digits` run starting at `start`, if there is one.
fn version_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut pos = start;
    for part in 0..3 {
        if part > 0 {
            if bytes.get(pos) != Some(&b'.') {
                return None;
            }
            pos += 1;
        }
        let digits = bytes[pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        pos += digits;
    }
    Some(pos)
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
    fn token_is_the_first_bounded_run_in_the_filename() {
        assert_eq!(
            version_token("c9300_image_16.9.4.SPA.bin"),
            Some("16.9.4".to_string())
        );
        assert_eq!(
            version_token("a_1.2.3.4_9.8.7.bin"),
            Some("9.8.7".to_string())
        );
        assert_eq!(version_token("1.2.3.4.bin"), None);
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
