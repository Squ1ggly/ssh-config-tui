//! Checks a value before it is written. Changes save the moment they are made,
//! and ssh refuses to run at all — for every host, not just this one — when the
//! config has a value it can't parse, so a bad value has to be caught here.
//!
//! The rules mirror ssh's own parsers in readconf.c and misc.c.

use crate::forward::is_port;
use crate::schema::{Keyword, Kind};

/// Checks `value` for the directive `keyword`. Empty values are always fine:
/// they mean "remove this line".
pub fn check(keyword: &Keyword, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    let name = keyword.name;
    match keyword.kind {
        _ if name == "Port" => {
            if !is_port(value) {
                return Err(format!(
                    "Port must be a number from 1 to 65535, not '{value}'"
                ));
            }
        }
        Kind::Int => {
            // atoi_err(): a non-negative decimal integer that fits in an int.
            if !value.chars().all(|c| c.is_ascii_digit()) || value.parse::<i32>().is_err() {
                return Err(format!("{name} must be a whole number, not '{value}'"));
            }
        }
        Kind::Time => {
            if value != "none" && !is_time(value) {
                return Err(format!(
                    "{name} takes a time like 30, 45s, 5m or 1h30m, not '{value}'"
                ));
            }
        }
        // A few choices also accept a free-form value (ForwardAgent takes a
        // socket path, AddKeysToAgent a lifetime), so those are let through.
        Kind::Flag | Kind::Choice
            if !keyword.values.iter().any(|v| v.eq_ignore_ascii_case(value))
                && !accepts_free_form(name) =>
        {
            return Err(format!(
                "{name} must be one of {}, not '{value}'",
                keyword.values.join(", ")
            ));
        }
        _ => {}
    }
    Ok(())
}

fn accepts_free_form(name: &str) -> bool {
    matches!(name, "ForwardAgent" | "AddKeysToAgent" | "ControlPersist")
}

/// ssh's `convtime()`: one or more `<number><unit>` groups where the unit is
/// s, m, h, d or w (or nothing, meaning seconds). Only seconds may be
/// fractional, and seconds may appear at most once.
pub fn is_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    let mut seen_seconds = false;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let number = &value[start..i];
        if number.is_empty() || number.matches('.').count() > 1 || number.ends_with('.') {
            return false;
        }
        let unit = bytes.get(i).map(|b| b.to_ascii_lowercase());
        let is_seconds = matches!(unit, None | Some(b's'));
        match unit {
            None | Some(b's' | b'm' | b'h' | b'd' | b'w') => {}
            _ => return false,
        }
        if is_seconds {
            if seen_seconds {
                return false;
            }
            seen_seconds = true;
        } else if number.contains('.') {
            return false;
        }
        if unit.is_some() {
            i += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::lookup;

    fn check_named(name: &str, value: &str) -> Result<(), String> {
        check(lookup(name).unwrap(), value)
    }

    #[test]
    fn times_follow_convtime() {
        for ok in [
            "30", "45s", "5m", "1h30m", "2d", "1w", "1.5", "1.5s", "10M", "1h30",
        ] {
            assert!(is_time(ok), "{ok} should be a valid time");
        }
        for bad in ["", "abc", "5x", "1.5m", "30s30s", "1.", "m", "1..5", "-5"] {
            assert!(!is_time(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn time_options_also_accept_none() {
        assert!(check_named("ConnectTimeout", "none").is_ok());
        assert!(check_named("ServerAliveInterval", "60").is_ok());
        let error = check_named("ServerAliveInterval", "sixty").unwrap_err();
        assert!(
            error.contains("1h30m"),
            "the message shows what is valid: {error}"
        );
    }

    #[test]
    fn integers_must_be_whole_non_negative_numbers() {
        assert!(check_named("ServerAliveCountMax", "3").is_ok());
        assert!(check_named("ServerAliveCountMax", "-1").is_err());
        assert!(check_named("ServerAliveCountMax", "3.5").is_err());
        assert!(check_named("ServerAliveCountMax", "99999999999").is_err());
    }

    #[test]
    fn port_is_checked_as_a_port() {
        assert!(check_named("Port", "22").is_ok());
        assert!(check_named("Port", "0").is_err());
        assert!(check_named("Port", "70000").is_err());
        assert!(check_named("Port", "http").is_err());
    }

    #[test]
    fn choices_must_be_one_of_the_allowed_values() {
        assert!(check_named("StrictHostKeyChecking", "accept-new").is_ok());
        assert!(check_named("StrictHostKeyChecking", "ACCEPT-NEW").is_ok());
        assert!(check_named("StrictHostKeyChecking", "maybe").is_err());
    }

    #[test]
    fn choices_with_a_free_form_spelling_are_not_rejected() {
        assert!(check_named("ForwardAgent", "~/.1password/agent.sock").is_ok());
        assert!(check_named("ControlPersist", "10m").is_ok());
    }

    #[test]
    fn rekey_limit_is_not_mistaken_for_a_time() {
        assert!(check_named("RekeyLimit", "1G 1h").is_ok());
    }

    #[test]
    fn empty_means_remove_and_is_always_allowed() {
        assert!(check_named("Port", "").is_ok());
        assert!(check_named("ServerAliveInterval", "  ").is_ok());
    }
}
