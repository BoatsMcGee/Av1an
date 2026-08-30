#[inline]
pub fn seconds_to_hms(seconds: u64, always_include_hours: bool) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;

    if hours > 0 || always_include_hours {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_only() {
        assert_eq!(seconds_to_hms(5, false), "00:05");
    }

    #[test]
    fn minutes_and_seconds() {
        assert_eq!(seconds_to_hms(130, false), "02:10");
    }

    #[test]
    fn hours_minutes_seconds() {
        assert_eq!(seconds_to_hms(3661, false), "01:01:01");
    }

    #[test]
    fn zero() {
        assert_eq!(seconds_to_hms(0, false), "00:00");
    }

    #[test]
    fn always_include_hours_true() {
        assert_eq!(seconds_to_hms(5, true), "00:00:05");
    }

    #[test]
    fn always_include_hours_false() {
        assert_eq!(seconds_to_hms(5, false), "00:05");
    }

    #[test]
    fn large_values() {
        assert_eq!(seconds_to_hms(90061, false), "25:01:01");
    }
}
