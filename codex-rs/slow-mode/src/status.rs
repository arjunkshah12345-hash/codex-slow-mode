use std::time::Duration;

/// Local slash-command arguments. None of these schedule a model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlowModeAction {
    On,
    Off,
    Status,
}

pub fn parse_slow_mode_args(args: &str) -> Result<SlowModeAction, String> {
    match args.trim().to_ascii_lowercase().as_str() {
        "" | "on" | "enable" | "enabled" => Ok(SlowModeAction::On),
        "off" | "disable" | "disabled" => Ok(SlowModeAction::Off),
        "status" => Ok(SlowModeAction::Status),
        other => Err(format!("Usage: /slow-mode [on|off|status] (got `{other}`)")),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlowModeStatus {
    pub enabled: bool,
    pub model: Option<String>,
    pub fast_mode: Option<bool>,
    pub primary_used_percent: Option<f64>,
    pub primary_resets_in: Option<Duration>,
    pub secondary_used_percent: Option<f64>,
    pub secondary_resets_in: Option<Duration>,
    pub estimated_request_cost_percent: Option<f64>,
    pub next_eligible_in: Option<Duration>,
    pub queued_model_requests: usize,
    pub preserving_secondary: bool,
    pub note: Option<String>,
}

pub fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 && minutes > 0 {
        format!("{hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h")
    } else if minutes > 0 && seconds > 0 && hours == 0 && minutes < 10 {
        format!("{minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

pub fn render_status(status: &SlowModeStatus) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Slow mode: {}",
        if status.enabled { "ON" } else { "OFF" }
    ));
    if let Some(model) = &status.model {
        lines.push(format!("Model: {model}"));
    }
    if let Some(fast) = status.fast_mode {
        lines.push(format!("Fast mode: {}", if fast { "ON" } else { "OFF" }));
    }
    if let Some(used) = status.primary_used_percent {
        lines.push(format!("Primary usage: {}%", format_percent(used)));
    }
    if let Some(reset) = status.primary_resets_in {
        lines.push(format!("Primary reset: {}", format_duration(reset)));
    }
    if let Some(used) = status.secondary_used_percent {
        lines.push(format!("Secondary usage: {}%", format_percent(used)));
    }
    if let Some(reset) = status.secondary_resets_in {
        lines.push(format!("Secondary reset: {}", format_duration(reset)));
    }
    if let Some(cost) = status.estimated_request_cost_percent {
        lines.push(format!(
            "Estimated current request cost: {}%",
            format_percent(cost)
        ));
    }
    if let Some(next) = status.next_eligible_in {
        lines.push(format!(
            "Next eligible inference: ~{}",
            format_duration(next)
        ));
    }
    lines.push(format!(
        "Queued model requests: {}",
        status.queued_model_requests
    ));
    if status.preserving_secondary {
        lines.push("Slow mode is preserving your longer-term allowance.".to_string());
    }
    if let Some(note) = &status.note {
        lines.push(note.clone());
    }
    lines.join("\n")
}

fn format_percent(value: f64) -> String {
    if !value.is_finite() {
        return "unavailable".to_string();
    }
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 0.05 {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn bare_and_on_enable() {
        assert_eq!(parse_slow_mode_args("").unwrap(), SlowModeAction::On);
        assert_eq!(parse_slow_mode_args("on").unwrap(), SlowModeAction::On);
        assert_eq!(parse_slow_mode_args("  ON ").unwrap(), SlowModeAction::On);
    }

    #[test]
    fn off_and_status_parse() {
        assert_eq!(parse_slow_mode_args("off").unwrap(), SlowModeAction::Off);
        assert_eq!(
            parse_slow_mode_args("status").unwrap(),
            SlowModeAction::Status
        );
    }

    #[test]
    fn unknown_args_do_not_default_to_enable() {
        assert!(parse_slow_mode_args("faster").is_err());
    }

    #[test]
    fn status_omits_unknown_fields() {
        let rendered = render_status(&SlowModeStatus {
            enabled: true,
            model: Some("gpt-5.4".to_string()),
            fast_mode: Some(false),
            primary_used_percent: Some(43.0),
            primary_resets_in: Some(Duration::from_secs(2 * 3600 + 51 * 60)),
            secondary_used_percent: None,
            secondary_resets_in: None,
            estimated_request_cost_percent: Some(0.8),
            next_eligible_in: Some(Duration::from_secs(73)),
            queued_model_requests: 1,
            preserving_secondary: false,
            note: None,
        });
        assert!(rendered.contains("Slow mode: ON"));
        assert!(rendered.contains("Model: gpt-5.4"));
        assert!(rendered.contains("Fast mode: OFF"));
        assert!(rendered.contains("Primary usage: 43%"));
        assert!(rendered.contains("Primary reset: 2h 51m"));
        assert!(rendered.contains("Estimated current request cost: 0.8%"));
        assert!(rendered.contains("Next eligible inference: ~1m 13s"));
        assert!(rendered.contains("Queued model requests: 1"));
        assert!(!rendered.contains("Secondary"));
    }
}
