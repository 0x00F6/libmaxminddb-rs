use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use crate::config::{IMPLEMENTATIONS, WRITER_IMPLEMENTATIONS};
use crate::metrics::{
    ANSI_CYAN, ANSI_MAGENTA, ANSI_RESET, LOWER_IS_BETTER, colorize, fmt_bytes, fmt_duration,
    fmt_rate, style_bold, style_cyan, style_gray, style_magenta, style_slowest, style_winner,
    style_yellow, supports_color,
};
use crate::stats::{extract_summary_items, find_winners_and_slowest};

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn visible_width(s: &str) -> usize {
    let clean = strip_ansi(s);
    let mut w = 0;
    for c in clean.chars() {
        if ('\u{fe00}'..='\u{fe0f}').contains(&c) || c == '\u{200d}' {
            continue;
        }
        w += c.width().unwrap_or(1);
    }
    w
}

pub fn pad_left_visible(s: &str, target_width: usize) -> String {
    let w = visible_width(s);
    if w >= target_width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(target_width - w), s)
    }
}

pub fn pad_right_visible(s: &str, target_width: usize) -> String {
    let w = visible_width(s);
    if w >= target_width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(target_width - w))
    }
}

pub fn render_banner(
    title: &str,
    subtitle: &str,
    width: usize,
    border_color: &str,
    use_color: bool,
) -> String {
    let b_col = if use_color { border_color } else { "" };
    let reset = if use_color { ANSI_RESET } else { "" };

    let top = format!("{b_col}╔{}╗{reset}", "═".repeat(width.saturating_sub(2)));
    let mid = format!("{b_col}╠{}╣{reset}", "═".repeat(width.saturating_sub(2)));
    let bot = format!("{b_col}╚{}╝{reset}", "═".repeat(width.saturating_sub(2)));

    let center = |text: &str| {
        let vlen = visible_width(text);
        let total_pad = width.saturating_sub(2 + vlen);
        let lpad = total_pad / 2;
        let rpad = total_pad - lpad;
        format!(
            "{b_col}║{reset}{}{text}{}{b_col}║{reset}",
            " ".repeat(lpad),
            " ".repeat(rpad)
        )
    };

    format!(
        "{}\n{}\n{}\n{}\n{}",
        top,
        center(title),
        mid,
        center(subtitle),
        bot
    )
}

pub fn print_console_summary(aggregated_rows: &[Value], use_color: Option<bool>) {
    let use_color = use_color.unwrap_or_else(supports_color);
    let items = extract_summary_items(aggregated_rows);
    if items.is_empty() {
        return;
    }

    let reader_items: Vec<_> = items
        .iter()
        .filter(|it| !it.scenario.starts_with("Writer"))
        .collect();

    let col_scen_w = 30;
    let col_met_w = 23;
    let col_impl_w = 18;
    let table_w = col_scen_w + col_met_w + col_impl_w * IMPLEMENTATIONS.len();

    let title_r = style_bold(
        "🏁 📊 Reader Benchmark Summary — Key Metrics & p99 Latency ⚡",
        use_color,
    );
    let sub_r = format!(
        "{}  {}  {}  {}  {}  {}  {}  {}  {}",
        style_winner("🦀 libmaxminddb-rs", use_color),
        style_gray("·", use_color),
        style_cyan("🏛️ libmaxminddb (C)", use_color),
        style_gray("·", use_color),
        style_yellow("📦 maxminddb-rust", use_color),
        style_gray("·", use_color),
        colorize("🚀 geoip2-rs", "\x1b[1;31m", use_color),
        style_gray("·", use_color),
        colorize("🐹 maxminddb-golang", "\x1b[1;36m", use_color),
    );

    println!(
        "\n{}",
        render_banner(&title_r, &sub_r, table_w, ANSI_CYAN, use_color)
    );

    let mut header = format!("{:<30} {:<23}", "Scenario", "Metric");
    for &name in IMPLEMENTATIONS {
        let short = match name {
            "libmaxminddb-rs" => "🦀 ours",
            "libmaxminddb" => "🏛️ C",
            "maxminddb-rust" => "📦 maxminddb",
            "geoip2-rs" => "🚀 geoip2",
            "maxminddb-golang" => "🐹 go",
            _ => name,
        };
        header.push_str(&pad_left_visible(short, col_impl_w));
    }
    println!("{}", style_bold(&header, use_color));
    println!("{}", style_gray(&"─".repeat(table_w), use_color));

    for item in &reader_items {
        let mut row = format!("{:<30} {:<23}", item.scenario, item.metric_label);

        // find winner/slowest with tie safety
        let mut vals: Vec<(&str, f64)> = Vec::new();
        for &name in IMPLEMENTATIONS {
            if let Some(r) = item.rows.get(name) {
                if let Some(v) = r.get(&item.metric_key).and_then(|x| x.as_f64()) {
                    if !v.is_nan() && (v > 0.0 || item.metric_key == "allocations_per_op") {
                        vals.push((name, v));
                    }
                }
            }
        }
        let (winners, slowest) = find_winners_and_slowest(&vals, item.direction == LOWER_IS_BETTER);

        for &name in IMPLEMENTATIONS {
            let cell = match item.rows.get(name) {
                Some(r) => {
                    if r.get("unsupported").and_then(|v| v.as_bool()) == Some(true) {
                        style_gray("⚠️ unsupported", use_color)
                    } else if let Some(n) = r.get(&item.metric_key).and_then(|v| v.as_f64()) {
                        let formatted =
                            if item.unit == "ns" || item.unit == "µs" || item.unit == "s" {
                                fmt_duration(Some(n))
                            } else if item.unit == "ops/s" {
                                fmt_rate(Some(n))
                            } else if item.unit == "allocs" {
                                format!("{n:.0}")
                            } else {
                                format!("{n:.1}")
                            };

                        if winners.contains(&name) {
                            style_winner(&format!("{formatted} 🏆"), use_color)
                        } else if slowest.contains(&name) {
                            style_slowest(&formatted, use_color)
                        } else {
                            formatted
                        }
                    } else {
                        style_gray("—", use_color)
                    }
                }
                None => style_gray("—", use_color),
            };
            row.push_str(&pad_left_visible(&cell, col_impl_w));
        }
        println!("{row}");
    }
    println!("{}", style_gray(&"═".repeat(table_w), use_color));

    // Writer summary if present
    let writer_items: Vec<_> = items
        .iter()
        .filter(|it| it.scenario.starts_with("Writer"))
        .collect();

    if !writer_items.is_empty() {
        let col_w_impl = 24;
        let w_table_w = col_scen_w + col_met_w + col_w_impl * WRITER_IMPLEMENTATIONS.len();
        let title_w = style_bold(
            "✍️ 📦 Writer Benchmark Summary — Performance & Memory Footprint 🚀",
            use_color,
        );
        let sub_w = format!(
            "{}  {}  {}",
            style_winner("🦀 libmaxminddb-rs", use_color),
            style_gray("·", use_color),
            style_magenta("✍️ mmdbwriter (Go)", use_color),
        );
        println!(
            "\n{}",
            render_banner(&title_w, &sub_w, w_table_w, ANSI_MAGENTA, use_color)
        );

        let mut w_header = format!("{:<30} {:<23}", "Scenario", "Metric");
        for &name in WRITER_IMPLEMENTATIONS {
            let label = if name == "libmaxminddb-rs" {
                "🦀 libmaxminddb-rs"
            } else {
                "✍️ mmdbwriter (Go)"
            };
            w_header.push_str(&pad_left_visible(label, col_w_impl));
        }
        println!("{}", style_bold(&w_header, use_color));
        println!("{}", style_gray(&"─".repeat(w_table_w), use_color));

        for item in &writer_items {
            let mut row = format!("{:<30} {:<23}", item.scenario, item.metric_label);

            let mut vals: Vec<(&str, f64)> = Vec::new();
            for &name in WRITER_IMPLEMENTATIONS {
                if let Some(r) = item.rows.get(name) {
                    if let Some(v) = r.get(&item.metric_key).and_then(|x| x.as_f64()) {
                        if !v.is_nan() && v > 0.0 {
                            vals.push((name, v));
                        }
                    }
                }
            }
            let (winners, slowest) =
                find_winners_and_slowest(&vals, item.direction == LOWER_IS_BETTER);

            for &name in WRITER_IMPLEMENTATIONS {
                let cell = match item.rows.get(name) {
                    Some(r) => {
                        if let Some(n) = r.get(&item.metric_key).and_then(|v| v.as_f64()) {
                            let formatted = if item.unit == "s" {
                                fmt_duration(Some(n))
                            } else if item.unit == "ops/s" {
                                fmt_rate(Some(n))
                            } else if item.unit == "MB" {
                                fmt_bytes(Some(n))
                            } else {
                                format!("{n:.1}")
                            };
                            if winners.contains(&name) {
                                style_winner(&format!("{formatted} 🏆"), use_color)
                            } else if slowest.contains(&name) {
                                style_slowest(&formatted, use_color)
                            } else {
                                formatted
                            }
                        } else {
                            style_gray("—", use_color)
                        }
                    }
                    None => style_gray("—", use_color),
                };
                row.push_str(&pad_left_visible(&cell, col_w_impl));
            }
            println!("{row}");
        }
        println!("{}", style_gray(&"═".repeat(w_table_w), use_color));
    }
}
