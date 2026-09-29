use std::collections::HashMap;

pub const LOWER_IS_BETTER: &str = "min";
pub const HIGHER_IS_BETTER: &str = "max";

pub const ANSI_RESET: &str = "\x1b[0m";
pub const ANSI_BOLD: &str = "\x1b[1m";
pub const ANSI_GREEN: &str = "\x1b[1;32m";
pub const ANSI_RED: &str = "\x1b[1;31m";
pub const ANSI_YELLOW: &str = "\x1b[1;33m";
pub const ANSI_GRAY: &str = "\x1b[90m";
pub const ANSI_CYAN: &str = "\x1b[1;36m";
pub const ANSI_MAGENTA: &str = "\x1b[1;35m";
pub const ANSI_BRIGHT_WHITE: &str = "\x1b[1;97m";

pub fn supports_color() -> bool {
    if std::env::var("NO_COLOR").is_ok() || std::env::var("TERM").as_deref() == Ok("dumb") {
        return false;
    }
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

pub fn colorize(text: &str, code: &str, enabled: bool) -> String {
    if !enabled {
        text.to_string()
    } else {
        format!("{code}{text}{ANSI_RESET}")
    }
}

pub fn style_winner(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_GREEN, enabled)
}

pub fn style_slowest(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_RED, enabled)
}

pub fn style_bold(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_BOLD, enabled)
}

pub fn style_gray(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_GRAY, enabled)
}

pub fn style_cyan(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_CYAN, enabled)
}

pub fn style_magenta(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_MAGENTA, enabled)
}

pub fn style_yellow(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_YELLOW, enabled)
}

pub fn style_bright_white(text: &str, enabled: bool) -> String {
    colorize(text, ANSI_BRIGHT_WHITE, enabled)
}

pub fn fmt_duration(ns: Option<f64>) -> String {
    match ns {
        None => "—".to_string(),
        Some(v) if v.is_nan() => "—".to_string(),
        Some(v) if v < 1_000.0 => format!("{v:.1} ns"),
        Some(v) if v < 1_000_000.0 => format!("{:.2} µs", v / 1_000.0),
        Some(v) if v < 1_000_000_000.0 => format!("{:.2} ms", v / 1_000_000.0),
        Some(v) => format!("{:.2} s", v / 1_000_000_000.0),
    }
}

pub fn fmt_bytes(b: Option<f64>) -> String {
    match b {
        None => "—".to_string(),
        Some(v) if v.is_nan() => "—".to_string(),
        Some(v) if v < 1024.0 => format!("{v:.0} B"),
        Some(v) if v < 1024.0 * 1024.0 => format!("{:.2} KiB", v / 1024.0),
        Some(v) if v < 1024.0 * 1024.0 * 1024.0 => format!("{:.2} MiB", v / (1024.0 * 1024.0)),
        Some(v) => format!("{:.2} GiB", v / (1024.0 * 1024.0 * 1024.0)),
    }
}

pub fn fmt_rate(ops_s: Option<f64>) -> String {
    match ops_s {
        None => "—".to_string(),
        Some(v) if v.is_nan() => "—".to_string(),
        Some(v) if v >= 1_000_000.0 => format!("{:.2} M ops/s", v / 1_000_000.0),
        Some(v) if v >= 1_000.0 => format!("{:.1} K ops/s", v / 1_000.0),
        Some(v) => format!("{v:.0} ops/s"),
    }
}

pub fn fmt_float(val: Option<f64>, decimals: usize) -> String {
    match val {
        None => "—".to_string(),
        Some(v) if v.is_nan() => "—".to_string(),
        Some(v) => format!("{v:.decimals$}"),
    }
}

pub fn fmt_size_tag(size: usize) -> String {
    if size >= 1_000_000 {
        if size % 1_000_000 == 0 {
            format!("{}M", size / 1_000_000)
        } else {
            format!("{:.1}M", size as f64 / 1_000_000.0)
        }
    } else if size >= 1_000 {
        if size % 1_000 == 0 {
            format!("{}K", size / 1_000)
        } else {
            format!("{:.1}K", size as f64 / 1_000.0)
        }
    } else {
        size.to_string()
    }
}

pub fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn find_best_and_worst(
    values: &HashMap<String, Option<f64>>,
    direction: &str,
) -> (Option<String>, Option<String>) {
    let mut valid: Vec<(String, f64)> = values
        .iter()
        .filter_map(|(k, v)| {
            v.and_then(|val| {
                if !val.is_nan() && val > 0.0 {
                    Some((k.clone(), val))
                } else {
                    None
                }
            })
        })
        .collect();

    if valid.is_empty() {
        return (None, None);
    }
    if valid.len() == 1 {
        return (Some(valid[0].0.clone()), None);
    }

    if direction == LOWER_IS_BETTER {
        valid.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    } else {
        valid.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    }

    let winner = valid.first().map(|x| x.0.clone());
    let slowest = valid.last().map(|x| x.0.clone());
    (winner, slowest)
}
