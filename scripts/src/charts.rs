//! SVG chart generators for libmaxminddb-rs benchmark reports.
//!
//! This module provides functions to generate various SVG charts using format! to avoid
//! Rust 2021 raw string literal prefix issues.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{
    DEFAULT_THREAD_COUNTS, IMPLEMENTATIONS, LOOKUP_API_OPERATIONS, MEMORY_IMPLEMENTATIONS,
    SCALING_DATABASE_SIZES, WRITER_SIZES, display_name, get_color,
};
use crate::metrics::{esc_html, fmt_duration, fmt_rate, fmt_size_tag};
use crate::stats::{
    get_concurrent_rows, get_largest_lookup, get_lookup_api_row, get_scaling_rows, get_writer_rows,
};

/// Generate a horizontal bar chart comparing implementations on a single metric
pub fn svg_bar_chart(
    title: &str,
    items: &[(&str, f64, &str)], // (label, value, formatted)
    width: usize,
    row_height: usize,
    better: &str, // "min" or "max"
    subtitle: &str,
) -> (String, String) {
    if items.is_empty() {
        return (
            title.to_string(),
            format!("<div class=\"empty\">No data for {}</div>", esc_html(title)),
        );
    }

    let top_margin = 85;
    let bottom_margin = 25;
    let label_width = 170;
    let value_width = 150;
    let chart_width = width.saturating_sub(label_width + value_width + 40);
    let height = top_margin + items.len() * row_height + bottom_margin;

    let max_v = items.iter().map(|x| x.1).fold(0.0_f64, f64::max);
    let winner_val = if better == "min" {
        items
            .iter()
            .map(|x| x.1)
            .filter(|v| *v > 0.0)
            .fold(f64::INFINITY, f64::min)
    } else {
        max_v
    };

    let mut svg = String::new();
    svg.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"100%\" height=\"{}\">\n", width, height, height));
    svg.push_str("<style>");
    svg.push_str("svg { background-color: #0d1117; font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, Helvetica, Arial, sans-serif; }");
    svg.push_str("text { fill: #c9d1d9; }");
    svg.push_str(".chart-title { fill: #f0f6fc; font-size: 20px; font-weight: 700; letter-spacing: -0.2px; }");
    svg.push_str(".chart-subtitle { fill: #8b949e; font-size: 14px; font-weight: 400; }");
    svg.push_str(".svg-bar-track { fill: #161b22; }");
    svg.push_str(".svg-value { fill: #f0f6fc; font-size: 13px; font-weight: 700; }");
    svg.push_str("</style>\n");
    svg.push_str(&format!(
        "<text x=\"20\" y=\"38\" class=\"chart-title\">{}</text>\n",
        esc_html(title)
    ));
    svg.push_str(&format!(
        "<text x=\"20\" y=\"62\" class=\"chart-subtitle\">{}</text>\n",
        esc_html(subtitle)
    ));

    for (i, &(label, val, formatted)) in items.iter().enumerate() {
        let y = top_margin + i * row_height;
        let color = get_color(label);
        let bar_w = if max_v > 0.0 {
            ((val / max_v) * chart_width as f64).round() as usize
        } else {
            0
        };
        let is_winner = val > 0.0 && (val - winner_val).abs() < 1e-6;
        let badge = if is_winner { " 🏆" } else { "" };

        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"13\" font-weight=\"600\" text-anchor=\"end\">{}{}</text>\n",
            label_width, y + 18, esc_html(display_name(label)), badge
        ));
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"26\" rx=\"4\" class=\"svg-bar-track\"/>\n",
            label_width + 15, y, chart_width
        ));
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"26\" rx=\"4\" fill=\"{}\" opacity=\"0.9\"><title>{}: {} ({})</title></rect>\n",
            label_width + 15, y, bar_w, color, esc_html(display_name(label)), esc_html(formatted), esc_html(title)
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" class=\"svg-value\">{}</text>\n",
            label_width + 15 + bar_w + 12,
            y + 18,
            esc_html(formatted)
        ));
    }

    svg.push_str("</svg>");
    (title.to_string(), svg)
}

/// Generate a line chart showing trends across categories
pub fn svg_line_chart(
    title: &str,
    x_labels: &[String],
    series: &[(&str, Vec<Option<f64>>)], // (series_name, values)
    width: usize,
    height: usize,
    y_unit: &str,
    subtitle: &str,
    force_zero: bool,
) -> (String, String) {
    let fmt = move |v: f64| format!("{:.2} {}", v, y_unit);
    svg_line_chart_formatted(
        title, x_labels, series, width, height, y_unit, subtitle, force_zero, &fmt,
    )
}

/// Line chart with a caller-supplied hover formatter so exact values can be
/// shown (e.g. MiB with full precision) instead of a fixed `{:.2}` label.
pub fn svg_line_chart_formatted(
    title: &str,
    x_labels: &[String],
    series: &[(&str, Vec<Option<f64>>)], // (series_name, values)
    width: usize,
    height: usize,
    y_unit: &str,
    subtitle: &str,
    force_zero: bool,
    fmt_value: &dyn Fn(f64) -> String,
) -> (String, String) {
    svg_line_chart_styled(
        title, x_labels, series, width, height, y_unit, subtitle, force_zero, fmt_value, false,
        false,
    )
}

// Memory curves may coincide at pixel precision. Complementary dash patterns
// and nested, hollow markers keep each library visible without moving its data.
fn memory_line_pattern(name: &str) -> &'static str {
    match name {
        "maxminddb-rust" => " stroke-dasharray=\"10 14\"",
        "geoip2-rs" => " stroke-dasharray=\"6 18\" stroke-dashoffset=\"-14\"",
        _ => "",
    }
}

fn memory_marker(name: &str, x: usize, y: usize, color: &str, title: &str) -> String {
    let title = if title.is_empty() {
        String::new()
    } else {
        format!("<title>{}</title>", esc_html(title))
    };
    let shape = match name {
        "libmaxminddb" => "<rect x=\"-6\" y=\"-6\" width=\"12\" height=\"12\"",
        "maxminddb-rust" => "<path d=\"M 0 -4 L 4 0 L 0 4 L -4 0 Z\"",
        "geoip2-rs" => "<circle r=\"2\"",
        _ => "<circle r=\"8\"",
    };
    let fill = if name == "geoip2-rs" { color } else { "none" };
    format!(
        "{shape} transform=\"translate({x} {y})\" fill=\"{fill}\" stroke=\"{color}\" stroke-width=\"1.8\" style=\"cursor:pointer\">{title}</{}>\n",
        if name == "libmaxminddb" {
            "rect"
        } else if name == "maxminddb-rust" {
            "path"
        } else {
            "circle"
        }
    )
}

// Keep the shared chart signature, with one additional rendering option.
#[allow(clippy::too_many_arguments)]
fn svg_line_chart_styled(
    title: &str,
    x_labels: &[String],
    series: &[(&str, Vec<Option<f64>>)],
    width: usize,
    height: usize,
    y_unit: &str,
    subtitle: &str,
    force_zero: bool,
    fmt_value: &dyn Fn(f64) -> String,
    distinguish_series: bool,
    interactive_api: bool,
) -> (String, String) {
    let top_margin = 85;
    let bottom_margin = 70;
    let left_margin = 85;
    let right_margin = if interactive_api {
        // Reserve enough viewBox space for the longest API label at 13 px.
        48 + series
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0)
            * 9
    } else if distinguish_series {
        // Leave room for public API names such as lookup_value_with_prefix.
        if series.iter().any(|(name, _)| name.len() > 20) {
            300
        } else {
            220
        }
    } else {
        170
    };
    let plot_w = width.saturating_sub(left_margin + right_margin);
    let plot_h = height.saturating_sub(top_margin + bottom_margin);

    let all_vals: Vec<f64> = series
        .iter()
        .flat_map(|(_, vals)| vals.iter().filter_map(|&v| v))
        .collect();

    if all_vals.is_empty() {
        let mut empty_svg = String::new();
        empty_svg.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} 180\" width=\"100%\" height=\"180\">\n", width));
        empty_svg.push_str("<style>svg { background-color: #0d1117; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; } text { fill: #8b949e; }</style>\n");
        empty_svg.push_str(&format!(
            "<rect width=\"100%\" height=\"100%\" fill=\"#0d1117\" rx=\"8\"/>\n"
        ));
        empty_svg.push_str(&format!("<text x=\"20\" y=\"38\" fill=\"#f0f6fc\" font-size=\"18\" font-weight=\"700\">{}</text>\n", esc_html(title)));
        empty_svg.push_str(&format!(
            "<text x=\"20\" y=\"62\" fill=\"#8b949e\" font-size=\"13\">{}</text>\n",
            esc_html(subtitle)
        ));
        empty_svg.push_str(&format!("<text x=\"50%\" y=\"120\" text-anchor=\"middle\" fill=\"#8b949e\" font-size=\"14\">⚠️ Aucun résultat mesuré disponible pour ce scénario</text>\n"));
        empty_svg.push_str("</svg>");
        return (title.to_string(), empty_svg);
    }

    let measured_max = all_vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut max_v = if force_zero {
        measured_max.max(1.0)
    } else {
        measured_max
    };
    let mut min_v = if force_zero {
        0.0
    } else {
        all_vals.iter().copied().fold(f64::INFINITY, f64::min)
    };
    if !force_zero {
        // Pad the measured range, including the constant-value case. Starting
        // a min reduction at zero would silently defeat zoom for positive RSS.
        let padding = (max_v - min_v).max(max_v.abs() * 0.01).max(1e-6) * 0.08;
        min_v -= padding;
        max_v += padding;
    }
    let span = if (max_v - min_v).abs() < 1e-6 {
        1.0
    } else {
        max_v - min_v
    };

    let mut svg = String::new();
    let attributes = if interactive_api {
        " class=\"lookup-api-chart\"".to_string()
    } else if distinguish_series {
        format!(" class=\"memory-chart\" data-y-min=\"{min_v}\" data-y-max=\"{max_v}\"")
    } else {
        String::new()
    };
    svg.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"100%\" height=\"{}\"{attributes}>\n", width, height, height));
    svg.push_str("<style>");
    svg.push_str("svg { background-color: #0d1117; font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, Helvetica, Arial, sans-serif; }");
    svg.push_str("text { fill: #c9d1d9; }");
    svg.push_str(".chart-title { fill: #f0f6fc; font-size: 20px; font-weight: 700; letter-spacing: -0.2px; }");
    svg.push_str(".chart-subtitle { fill: #8b949e; font-size: 14px; font-weight: 400; }");
    svg.push_str(".axis-line { stroke: #30363d; stroke-width: 1.5; }");
    svg.push_str(".grid-line { stroke: #21262d; stroke-dasharray: 4,4; stroke-width: 1; }");
    svg.push_str(".axis-label { fill: #8b949e; font-size: 13px; }");
    svg.push_str(".svg-value { fill: #f0f6fc; font-size: 13px; font-weight: 700; }");
    svg.push_str(".legend-text { fill: #c9d1d9; font-size: 13px; font-weight: 600; }");
    if interactive_api {
        svg.push_str(".api-series { transition: opacity .12s ease; }.api-series.is-muted { opacity: .14; }.api-series.is-active .api-line { stroke-width: 5; }.api-series.is-active .legend-text { fill: #fff; font-weight: 800; }.api-series.is-active .api-legend-marker { stroke: #fff; stroke-width: 2; }.api-legend { cursor: pointer; outline: none; }.api-legend:focus-visible .legend-text { text-decoration: underline; }.api-hit-area { cursor: pointer; }");
    }
    if distinguish_series {
        svg.push_str(".memory-chart .memory-series { outline: none; }.memory-chart:has(.memory-series:hover, .memory-series:focus-visible) .memory-series:not(:hover):not(:focus-visible) { opacity: .15; }");
    }
    svg.push_str("</style>\n");
    svg.push_str(&format!(
        "<text x=\"20\" y=\"38\" class=\"chart-title\">{}</text>\n",
        esc_html(title)
    ));
    svg.push_str(&format!(
        "<text x=\"20\" y=\"62\" class=\"chart-subtitle\">{}</text>\n",
        esc_html(subtitle)
    ));

    // Draw axes
    svg.push_str(&format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" class=\"axis-line\"/>\n",
        left_margin,
        top_margin + plot_h,
        left_margin + plot_w,
        top_margin + plot_h
    ));
    svg.push_str(&format!(
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" class=\"axis-line\"/>\n",
        left_margin,
        top_margin,
        left_margin,
        top_margin + plot_h
    ));

    // Y ticks
    for i in 0..=5 {
        let frac = i as f64 / 5.0;
        let y_val = min_v + frac * span;
        let y_pos = top_margin + plot_h - (frac * plot_h as f64).round() as usize;
        svg.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" class=\"grid-line\"/>\n",
            left_margin,
            y_pos,
            left_margin + plot_w,
            y_pos
        ));
        let precision = if force_zero {
            1
        } else if span < 1.0 {
            3
        } else {
            2
        };
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" class=\"axis-label\" text-anchor=\"end\">{:.*} {}</text>\n",
            left_margin - 10,
            y_pos + 4,
            precision,
            y_val,
            esc_html(y_unit)
        ));
    }

    // X labels
    let n_x = x_labels.len();
    for (i, xlab) in x_labels.iter().enumerate() {
        let x_pos = if n_x > 1 {
            left_margin + (i * plot_w) / (n_x - 1)
        } else {
            left_margin + plot_w / 2
        };
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" class=\"axis-label\" text-anchor=\"middle\">{}</text>\n",
            x_pos,
            top_margin + plot_h + 25,
            esc_html(xlab)
        ));
    }

    // Series lines and legend
    for (s_idx, &(s_name, ref vals)) in series.iter().enumerate() {
        let color = get_color(s_name);
        let pattern = if distinguish_series {
            memory_line_pattern(s_name)
        } else {
            ""
        };
        if interactive_api {
            svg.push_str(&format!(
                "<g class=\"api-series\" data-series=\"{}\">\n",
                esc_html(s_name)
            ));
        } else if distinguish_series {
            svg.push_str(&format!("<g class=\"memory-series\" data-series=\"{}\" tabindex=\"0\" role=\"group\" aria-label=\"{} — hover or focus to highlight\">\n", esc_html(s_name), esc_html(display_name(s_name))));
        }
        let mut points = Vec::new();

        for (i, val) in vals.iter().enumerate() {
            if let Some(v) = *val {
                let x_pos = if n_x > 1 {
                    left_margin + (i * plot_w) / (n_x - 1)
                } else {
                    left_margin + plot_w / 2
                };
                let frac = (v - min_v) / span;
                let y_pos = top_margin + plot_h - (frac * plot_h as f64).round() as usize;
                points.push((i, x_pos, y_pos));
            }
        }

        // A missing observation breaks the line, rather than implying a measured
        // interpolation between points on either side of the gap.
        for segment in points.chunk_by(|a, b| a.0 + 1 == b.0) {
            if segment.len() > 1 {
                let pts_str = segment
                    .iter()
                    .map(|(_, x, y)| format!("{x},{y}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                if interactive_api {
                    svg.push_str(&format!("<polyline class=\"api-hit-area\" points=\"{pts_str}\" fill=\"none\" stroke=\"transparent\" stroke-width=\"16\" pointer-events=\"stroke\"/>\n"));
                    svg.push_str(&format!("<polyline class=\"api-line\" points=\"{pts_str}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2.5\" stroke-linejoin=\"round\" pointer-events=\"none\"/>\n"));
                } else {
                    svg.push_str(&format!("<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2.5\" stroke-linejoin=\"round\"{pattern}/>\n", pts_str, color));
                }
            }
        }

        for (idx, x, y) in &points {
            let val_label = vals
                .get(*idx)
                .and_then(|v| *v)
                .map(|v| fmt_value(v))
                .unwrap_or_default();
            let x_lbl = x_labels.get(*idx).cloned().unwrap_or_default();
            if distinguish_series {
                svg.push_str(&memory_marker(
                    s_name,
                    *x,
                    *y,
                    color,
                    &format!("{}: {} @ {}", display_name(s_name), val_label, x_lbl),
                ));
            } else {
                svg.push_str(&format!(
                "<circle class=\"{}\" cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{}\" style=\"cursor:pointer;\"><title>{}: {} @ {}</title></circle>\n",
                if interactive_api { "api-marker" } else { "" }, x, y, color, esc_html(display_name(s_name)), esc_html(&val_label), esc_html(&x_lbl)
                ));
            }
        }

        // Legend item
        let leg_x = left_margin + plot_w + 20;
        let leg_y = top_margin + 20 + s_idx * 24;
        if interactive_api {
            svg.push_str(&format!("<g class=\"api-legend\" tabindex=\"0\" role=\"button\" aria-pressed=\"false\" aria-label=\"Highlight {}\">\n", esc_html(display_name(s_name))));
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"22\" fill=\"transparent\"/>\n",
                leg_x - 8,
                leg_y - 15,
                right_margin - 16
            ));
        }
        if distinguish_series {
            svg.push_str(&format!("<line x1=\"{leg_x}\" y1=\"{leg_y}\" x2=\"{}\" y2=\"{leg_y}\" stroke=\"{color}\" stroke-width=\"2.5\"{pattern}/>\n", leg_x + 32));
            svg.push_str(&memory_marker(s_name, leg_x + 16, leg_y, color, ""));
        } else {
            svg.push_str(&format!(
                "<circle class=\"{}\" cx=\"{}\" cy=\"{}\" r=\"5\" fill=\"{}\"/>\n",
                if interactive_api {
                    "api-legend-marker"
                } else {
                    ""
                },
                leg_x,
                leg_y,
                color
            ));
        }
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" class=\"legend-text\">{}</text>\n",
            leg_x + if distinguish_series { 42 } else { 12 },
            leg_y + 4,
            esc_html(display_name(s_name))
        ));
        if interactive_api {
            svg.push_str("</g>\n</g>\n");
        } else if distinguish_series {
            svg.push_str("</g>\n");
        }
    }

    if distinguish_series {
        svg.push_str(&format!("<text x=\"{left_margin}\" y=\"{}\" class=\"axis-label\">Overlapping curves use distinct lines and markers. Hover a legend or use Tab to highlight.</text>\n", height.saturating_sub(15)));
    }
    svg.push_str("</svg>");
    (title.to_string(), svg)
}

/// Generate a candlestick chart showing percentile distribution
pub fn svg_candlestick_chart(
    title: &str,
    items: &[(&str, CandlestickStats)],
    width: usize,
    row_height: usize,
    subtitle: &str,
) -> (String, String) {
    if items.is_empty() {
        return (
            title.to_string(),
            format!("<div class=\"empty\">No data for {}</div>", esc_html(title)),
        );
    }

    let mut sorted_items: Vec<_> = items.iter().collect();
    sorted_items.sort_by(|a, b| a.1.p99_ns.partial_cmp(&b.1.p99_ns).unwrap());

    let all_p99: Vec<f64> = sorted_items.iter().map(|(_, s)| s.p99_ns).collect();
    let max_val = all_p99.iter().fold(0.0_f64, |a, &b| a.max(b)) * 1.15;
    if max_val <= 0.0 {
        return (
            title.to_string(),
            format!(
                "<div class=\"empty\">Invalid data for {}</div>",
                esc_html(title)
            ),
        );
    }

    let pad_left = 28;
    let left = 260;
    let right = 420;
    let chart_w = width.saturating_sub(left + right);
    let _title_y = 36;
    let _subtitle_y = 66;
    let header_h = 130;
    let plot_bot = header_h + row_height * sorted_items.len();
    let height = plot_bot + 48;

    let mut svg = String::new();
    svg.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"100%\" height=\"{}\">\n", width, height, height));
    svg.push_str("<style>");
    svg.push_str("svg { background-color: #0d1117; font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, Helvetica, Arial, sans-serif; }");
    svg.push_str("text { fill: #c9d1d9; }");
    svg.push_str(".chart-title { fill: #f0f6fc; font-size: 20px; font-weight: 700; letter-spacing: -0.2px; }");
    svg.push_str(".chart-subtitle { fill: #8b949e; font-size: 14px; font-weight: 400; }");
    svg.push_str(".axis-line { stroke: #30363d; stroke-width: 1.5; }");
    svg.push_str(".grid-line { stroke: #21262d; stroke-dasharray: 4,4; stroke-width: 1; }");
    svg.push_str(".axis-label { fill: #8b949e; font-size: 13px; }");
    svg.push_str(".svg-bar-track { fill: #161b22; }");
    svg.push_str(".svg-value { fill: #f0f6fc; font-size: 13px; font-weight: 700; }");
    svg.push_str(".legend-text { fill: #c9d1d9; font-size: 13px; font-weight: 600; }");
    svg.push_str(".legend-box { fill: #161b22; stroke: #30363d; stroke-width: 1; }");
    svg.push_str(".legend-label { fill: #8b949e; font-size: 11px; font-weight: 700; letter-spacing: 0.8px; }");
    svg.push_str("</style>\n");
    svg.push_str(&format!(
        "<rect width=\"{}\" height=\"{}\" fill=\"#0d1117\" rx=\"10\"/>\n",
        width, height
    ));
    svg.push_str(&format!(
        "<text x=\"20\" y=\"38\" class=\"chart-title\">{}</text>\n",
        esc_html(title)
    ));
    svg.push_str(&format!(
        "<text x=\"20\" y=\"62\" class=\"chart-subtitle\">{}</text>\n",
        esc_html(subtitle)
    ));

    // Candlestick Anatomy Legend
    let leg_w = 540;
    svg.push_str(&format!(
        "<g class=\"chart-legend\" transform=\"translate({}, {})\">\n",
        pad_left, 92
    ));
    svg.push_str(&format!(
        "<rect class=\"legend-box\" x=\"0\" y=\"-14\" width=\"{}\" height=\"28\" rx=\"6\"/>\n",
        leg_w
    ));
    svg.push_str("<text x=\"14\" y=\"4\" class=\"legend-label\">ANATOMY</text>\n");
    svg.push_str(
        "<line x1=\"88\" y1=\"-5\" x2=\"88\" y2=\"5\" stroke=\"#8b949e\" stroke-width=\"2\"/>\n",
    );
    svg.push_str(
        "<line x1=\"88\" y1=\"0\" x2=\"114\" y2=\"0\" stroke=\"#8b949e\" stroke-width=\"1.5\"/>\n",
    );
    svg.push_str(
        "<text x=\"122\" y=\"4\" class=\"legend-text\" style=\"font-size:12px;\">Min</text>\n",
    );
    svg.push_str("<rect x=\"162\" y=\"-8\" width=\"36\" height=\"16\" rx=\"3\" fill=\"#00f5a0\" fill-opacity=\"0.45\" stroke=\"#00f5a0\" stroke-width=\"1.5\"/>\n");
    svg.push_str(
        "<line x1=\"162\" y1=\"-8\" x2=\"162\" y2=\"8\" stroke=\"#ffffff\" stroke-width=\"2\"/>\n",
    );
    svg.push_str("<text x=\"208\" y=\"4\" class=\"legend-text\" style=\"font-size:12px;\">p50 to p95 (Body)</text>\n");
    svg.push_str(
        "<line x1=\"395\" y1=\"0\" x2=\"425\" y2=\"0\" stroke=\"#8b949e\" stroke-width=\"1.5\"/>\n",
    );
    svg.push_str("<circle cx=\"425\" cy=\"0\" r=\"4\" fill=\"#ffffff\" stroke=\"#00f5a0\" stroke-width=\"1.5\"/>\n");
    svg.push_str(
        "<text x=\"436\" y=\"4\" class=\"legend-text\" style=\"font-size:12px;\">p99 Tail</text>\n",
    );
    svg.push_str("</g>\n");

    // Grid vertical lines
    let num_ticks = 4;
    for i in 0..=num_ticks {
        let tick_val = i as f64 * (max_val / num_ticks as f64);
        let gx = left + ((tick_val / max_val) * chart_w as f64).round() as usize;
        svg.push_str(&format!(
            "<line class=\"grid-line\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>\n",
            gx, header_h, gx, plot_bot
        ));
        svg.push_str(&format!(
            "<text class=\"axis-label\" style=\"font-size:12px;\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>\n",
            gx, plot_bot + 20, fmt_duration(Some(tick_val))
        ));
    }

    for (i, (label, stats)) in sorted_items.iter().enumerate() {
        let y = header_h + i * row_height;
        let color = get_color(label);
        let is_winner = i == 0;

        let min_v = stats.min_ns;
        let p50_v = stats.p50_ns;
        let p95_v = stats.p95_ns;
        let p99_v = stats.p99_ns;
        let max_v = stats.max_ns;

        let x_min = left + ((min_v / max_val) * chart_w as f64).round() as usize;
        let x_p50 = left + ((p50_v / max_val) * chart_w as f64).round() as usize;
        let x_p95 = left + ((p95_v / max_val) * chart_w as f64).round() as usize;
        let x_p99 = left + ((p99_v / max_val) * chart_w as f64).round() as usize;

        let mid_y = y + 20;

        // Rank badge
        if is_winner {
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"46\" height=\"24\" rx=\"4\" fill=\"rgba(0,245,160,0.15)\" stroke=\"#00f5a0\" stroke-width=\"1.5\"/>\n",
                pad_left, y + 8
            ));
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" style=\"fill:#00f5a0;font-weight:700;font-size:12px;\">#1 🏆</text>\n",
                pad_left + 23, y + 24
            ));
        } else {
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"36\" height=\"24\" rx=\"4\" fill=\"#21262d\"/>\n",
                pad_left,
                y + 8
            ));
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" style=\"fill:#8b949e;font-weight:600;font-size:12px;\">#{}</text>\n",
                pad_left + 18, y + 24, i + 1
            ));
        }

        // Implementation name
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" class=\"axis-label\" style=\"fill:{};font-weight:700;font-size:14px;\">{}</text>\n",
            84, y + 25, color, esc_html(display_name(label))
        ));

        // Track
        svg.push_str(&format!(
            "<rect class=\"svg-bar-track\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"24\" rx=\"4\" opacity=\"0.5\"/>\n",
            left, y + 8, chart_w
        ));

        // Candlestick components
        let body_w = if x_p95 > x_p50 { x_p95 - x_p50 } else { 4 };
        let opacity = if is_winner { 0.55 } else { 0.35 };
        let stroke_width = if is_winner { 2.5 } else { 1.5 };
        let circle_r = if is_winner { 5 } else { 4 };

        // Lower wick (min)
        svg.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"2\" stroke-opacity=\"0.75\"/>\n",
            x_min, mid_y, x_p50, mid_y, color
        ));
        svg.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"2\"/>\n",
            x_min,
            mid_y - 7,
            x_min,
            mid_y + 7,
            color
        ));

        // Body (p50 to p95)
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"26\" rx=\"4\" fill=\"{}\" fill-opacity=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
            x_p50, y + 7, body_w, color, opacity, color, stroke_width
        ));

        // White line in body
        svg.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#ffffff\" stroke-width=\"2\"/>\n",
            x_p50, y + 7, x_p50, y + 33
        ));

        // Upper wick (p95 to p99)
        svg.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"2\" stroke-opacity=\"0.75\"/>\n",
            x_p95, mid_y, x_p99, mid_y, color
        ));

        // p99 marker
        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#ffffff\" stroke=\"{}\" stroke-width=\"2\"/>\n",
            x_p99, mid_y, circle_r, color
        ));

        // Value labels (Min, p50, p95, p99)
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" class=\"svg-value\" style=\"font-size:12px;\">Min: {} │ p50: {} │ p95: {} │ <tspan style=\"fill:{};font-weight:700;\">p99: {}</tspan></text>\n",
            left + chart_w + 16, y + 24,
            fmt_duration(Some(min_v)),
            fmt_duration(Some(p50_v)),
            fmt_duration(Some(p95_v)),
            color,
            fmt_duration(Some(p99_v))
        ));
        svg.push_str(&format!(
            "<title>{}: Min={} | p50={} | p95={} | p99={} | Max={}</title>\n",
            esc_html(display_name(label)),
            fmt_duration(Some(min_v)),
            fmt_duration(Some(p50_v)),
            fmt_duration(Some(p95_v)),
            fmt_duration(Some(p99_v)),
            fmt_duration(Some(max_v))
        ));

        // Tooltip
        svg.push_str(&format!(
            "<title>{} (Rank #{}): Min={} | p50={} | p95={} | p99={} | Max={}</title>\n",
            esc_html(display_name(label)),
            i + 1,
            fmt_duration(Some(min_v)),
            fmt_duration(Some(p50_v)),
            fmt_duration(Some(p95_v)),
            fmt_duration(Some(p99_v)),
            fmt_duration(Some(max_v))
        ));
    }

    svg.push_str("</svg>");
    (title.to_string(), svg)
}

/// Struct for candlestick data - defined here to avoid circular dependencies
#[derive(Clone, Debug)]
pub struct CandlestickStats {
    pub label: String,
    pub min_ns: f64,
    pub p50_ns: f64,
    pub p95_ns: f64,
    pub p99_ns: f64,
    pub max_ns: f64,
    pub color: String,
}

/// Generate a grouped bar chart for multi-series comparisons
pub fn svg_grouped_bar_chart(
    title: &str,
    categories: &[String],
    series: &[(&str, Vec<Option<f64>>)],
    formatter: fn(f64) -> String,
    width: usize,
    height: usize,
    subtitle: &str,
) -> (String, String) {
    let all_vals: Vec<f64> = series
        .iter()
        .flat_map(|(_, vals)| vals.iter().filter_map(|&v| v))
        .collect();

    if all_vals.is_empty() {
        return (
            title.to_string(),
            format!("<div class=\"empty\">No data for {}</div>", esc_html(title)),
        );
    }

    let pad_left = 28;
    let left = 110;
    let right = 40;

    let ordered_series_names: Vec<&str> = IMPLEMENTATIONS
        .iter()
        .filter(|n| series.iter().any(|(name, _)| name == *n))
        .copied()
        .collect();

    let mut legend_svg = String::new();
    let mut leg_x = pad_left;
    for name in &ordered_series_names {
        let c = get_color(name);
        let item_w = display_name(name).len() * 8 + 36;
        if leg_x + item_w > width.saturating_sub(right) {
            leg_x = pad_left;
        }
        legend_svg.push_str(&format!(
            "<rect x=\"{}\" y=\"72\" width=\"14\" height=\"14\" rx=\"3\" fill=\"{}\"/>\n",
            leg_x, c
        ));
        legend_svg.push_str(&format!(
            "<text x=\"{}\" y=\"84\" class=\"legend-text\">{}</text>\n",
            leg_x + 20,
            esc_html(display_name(name))
        ));
        leg_x += item_w;
    }

    let top = 100;
    let bottom = height - 60;
    let plot_w = width.saturating_sub(left + right);
    let plot_h = bottom.saturating_sub(top);

    let min_y = 0.0;
    let max_y = all_vals.iter().copied().fold(0.0_f64, f64::max);
    if max_y <= min_y {
        return (
            title.to_string(),
            "<svg>Invalid chart data</svg>".to_string(),
        );
    }
    let span = max_y - min_y;

    let mut svg = String::new();
    svg.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"100%\" height=\"{}\">\n", width, height, height));
    svg.push_str("<style>");
    svg.push_str("svg { background-color: #0d1117; font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, Helvetica, Arial, sans-serif; }");
    svg.push_str("text { fill: #c9d1d9; }");
    svg.push_str(".chart-title { fill: #f0f6fc; font-size: 20px; font-weight: 700; letter-spacing: -0.2px; }");
    svg.push_str(".chart-subtitle { fill: #8b949e; font-size: 14px; font-weight: 400; }");
    svg.push_str(".axis-line { stroke: #30363d; stroke-width: 1.5; }");
    svg.push_str(".grid-line { stroke: #21262d; stroke-dasharray: 4,4; stroke-width: 1; }");
    svg.push_str(".axis-label { fill: #8b949e; font-size: 13px; }");
    svg.push_str("</style>\n");
    svg.push_str(&format!(
        "<rect width=\"{}\" height=\"{}\" fill=\"#0d1117\" rx=\"10\"/>\n",
        width, height
    ));
    svg.push_str(&format!(
        "<text x=\"20\" y=\"38\" class=\"chart-title\">{}</text>\n",
        esc_html(title)
    ));
    svg.push_str(&format!(
        "<text x=\"20\" y=\"62\" class=\"chart-subtitle\">{}</text>\n",
        esc_html(subtitle)
    ));
    svg.push_str(&legend_svg);

    // Y-axis ticks
    let num_ticks = 5;
    for i in 0..=num_ticks {
        let frac = i as f64 / num_ticks as f64;
        let y_val = min_y + frac * span;
        let y_pos = top + ((1.0 - frac) * plot_h as f64).round() as usize;
        svg.push_str(&format!(
            "<line class=\"grid-line\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>\n",
            left,
            y_pos,
            left + plot_w,
            y_pos
        ));
        svg.push_str(&format!(
            "<text class=\"axis-label\" x=\"{}\" y=\"{}\" text-anchor=\"end\">{}{}</text>\n",
            left - 14,
            y_pos + 4,
            (formatter)(y_val),
            esc_html("")
        ));
    }

    svg.push_str(&format!(
        "<line class=\"axis-line\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>\n",
        left,
        bottom,
        left + plot_w,
        bottom
    ));

    // X labels and bars
    let n_cat = categories.len();
    for (i, cat) in categories.iter().enumerate() {
        let cat_cx = left
            + if n_cat > 1 {
                (i * plot_w) / (n_cat - 1)
            } else {
                plot_w / 2
            };
        svg.push_str(&format!(
            "<text class=\"axis-label\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>\n",
            cat_cx,
            bottom + 25,
            esc_html(cat)
        ));

        let num_series = ordered_series_names.len();
        let bar_w = if num_series > 0 {
            (plot_w as f64 / n_cat as f64) * 0.75 / num_series as f64
        } else {
            0.0
        };
        let group_start = cat_cx as f64 - (num_series as f64 * bar_w) / 2.0;

        for (s_idx, name) in ordered_series_names.iter().enumerate() {
            let series_vals = series
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            if let Some(val) = series_vals.get(i).and_then(|v| *v) {
                let x = (group_start + s_idx as f64 * bar_w).round() as usize;
                let h = ((val - min_y) / span * plot_h as f64).round() as usize;
                let y = bottom - h;
                let c = get_color(name);

                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{:.1}\" height=\"{}\" fill=\"{}\" opacity=\"0.85\"/>\n",
                    x, y, bar_w.max(3.0), h, c
                ));
            }
        }
    }

    svg.push_str("</svg>");
    (title.to_string(), svg)
}

/// Shared renderer for the standalone SVGs and the HTML report.
pub fn memory_charts(aggregated: &[Value]) -> Vec<(&'static str, String)> {
    let x_labels: Vec<String> = SCALING_DATABASE_SIZES
        .iter()
        .map(|s| fmt_size_tag(*s))
        .collect();
    [
        (
            "rss_after_open_bytes",
            "memory-rss-after-open.svg",
            "RSS After Open (mmap)",
            "Median of 3 isolated processes · Zoomed Y axis (not zero-based)",
        ),
        (
            "rss_peak_bytes",
            "memory-rss-peak.svg",
            "Peak RSS During Lookups (mmap)",
            "Maximum of 3 kernel peaks · reset after open · 1M lookups, 50% hits / 50% misses",
        ),
    ]
    .into_iter()
    .map(|(metric, file, title, subtitle)| {
        let series: Vec<_> = MEMORY_IMPLEMENTATIONS
            .iter()
            .map(|&name| {
                let values = SCALING_DATABASE_SIZES
                    .iter()
                    .map(|&size| {
                        crate::memory::cell(aggregated, name, size, metric)
                            .ok()
                            .map(|bytes| bytes / 1_048_576.0)
                    })
                    .collect();
                (name, values)
            })
            .collect();
        let (_, svg) = svg_line_chart_styled(
            title,
            &x_labels,
            &series,
            1100,
            500,
            "MiB",
            subtitle,
            metric != "rss_after_open_bytes",
            &|mib| format!("{mib} MiB ({:.0} bytes)", mib * 1_048_576.0),
            true,
            false,
        );
        (file, svg)
    })
    .collect()
}

pub fn generate_memory_svg_charts(aggregated: &[Value], charts_dir: &Path) -> Vec<PathBuf> {
    if fs::create_dir_all(charts_dir).is_err() {
        return Vec::new();
    }
    memory_charts(aggregated)
        .into_iter()
        .filter_map(|(file, svg)| {
            let path = charts_dir.join(file);
            fs::write(&path, svg).ok().map(|_| path)
        })
        .collect()
}

/// Worker scaling of all reader lookup APIs on the same address stream.
pub fn lookup_api_charts(aggregated: &[Value]) -> Vec<(String, String)> {
    let mut charts = Vec::new();
    for family in ["ipv4", "ipv6"] {
        let x_labels: Vec<String> = DEFAULT_THREAD_COUNTS
            .iter()
            .map(|threads| format!("{threads}T"))
            .collect();
        let series: Vec<(&str, Vec<Option<f64>>)> = LOOKUP_API_OPERATIONS
            .iter()
            .map(|&name| {
                let values = DEFAULT_THREAD_COUNTS
                    .iter()
                    .map(|&threads| {
                        get_lookup_api_row(aggregated, family, threads, name)
                            .and_then(|r| r.get("throughput_ops_s"))
                            .and_then(Value::as_f64)
                            .map(|rate| rate / 1_000_000.0)
                    })
                    .collect();
                (name, values)
            })
            .collect();
        if series
            .iter()
            .any(|(_, values)| values.iter().any(Option::is_some))
        {
            let (_, svg) = svg_line_chart_styled(
                &format!("{family} random lookup — worker scaling"),
                &x_labels,
                &series,
                1300,
                560,
                "M ops/s",
                "Same 1M IPs and hit checksum; API work differs (lookup_many uses one IP)",
                true,
                &|rate| format!("{rate:.2} M ops/s"),
                false,
                true,
            );
            charts.push((format!("lookup-api-{family}-multithread.svg"), svg));
        }
    }
    charts
}

/// Generate all standalone SVG charts
pub fn generate_standalone_svg_charts(aggregated: &[Value], charts_dir: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(charts_dir).unwrap_or_default();
    let mut generated = Vec::new();

    for (file, svg) in lookup_api_charts(aggregated) {
        let path = charts_dir.join(file);
        if fs::write(&path, svg).is_ok() {
            generated.push(path);
        }
    }

    // 1. IPv4 Random Lookup Latency
    let ipv4_rows = get_largest_lookup(aggregated, "ipv4", "random");
    if !ipv4_rows.is_empty() {
        let mut items = Vec::new();
        for r in &ipv4_rows {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(p99) = r.get("p99_ns").and_then(|v| v.as_f64()) {
                items.push((impl_name, p99, fmt_duration(Some(p99))));
            }
        }
        let (_, svg) = svg_bar_chart(
            "IPv4 Lookup Latency (p99)",
            &items
                .iter()
                .map(|(a, b, c)| (*a, *b, c.as_str()))
                .collect::<Vec<_>>(),
            1000,
            48,
            "min",
            "p99 tail latency for 1M random lookups (lower is better)",
        );
        let path = charts_dir.join("lookup-latency-ipv4.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);
    }

    // 2. IPv6 Random Lookup Latency
    let ipv6_rows = get_largest_lookup(aggregated, "ipv6", "random");
    if !ipv6_rows.is_empty() {
        let mut items = Vec::new();
        for r in &ipv6_rows {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(p99) = r.get("p99_ns").and_then(|v| v.as_f64()) {
                items.push((impl_name, p99, fmt_duration(Some(p99))));
            }
        }
        let (_, svg) = svg_bar_chart(
            "IPv6 Lookup Latency (p99)",
            &items
                .iter()
                .map(|(a, b, c)| (*a, *b, c.as_str()))
                .collect::<Vec<_>>(),
            1000,
            48,
            "min",
            "p99 tail latency for 1M random lookups (lower is better)",
        );
        let path = charts_dir.join("lookup-latency-ipv6.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);
    }

    // 3. Multi-threaded Throughput
    let conc_rows = get_concurrent_rows(aggregated, "ipv4");
    if !conc_rows.is_empty() {
        let threads = [1, 4, 8, 16];
        let x_labels: Vec<String> = threads.iter().map(|t| format!("{t}T")).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &t in &threads {
                let v = conc_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("threads").and_then(|x| x.as_u64()) == Some(t as u64)
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "Peak Concurrent Throughput — 16 Threads",
            &x_labels,
            &series,
            1000,
            500,
            "M ops/s",
            "IPv4 throughput by thread count, 1 to 16 threads (higher is better)",
            true,
        );
        let path = charts_dir.join("concurrent-throughput.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);
    }

    // 4. Database size scaling
    let scale_rows = get_scaling_rows(aggregated);
    if !scale_rows.is_empty() {
        let sizes = SCALING_DATABASE_SIZES;
        let x_labels: Vec<String> = sizes.iter().map(|s| fmt_size_tag(*s)).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = scale_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("database_size").and_then(|x| x.as_u64()) == Some(sz as u64)
                    })
                    .and_then(|r| r.get("p99_ns").and_then(|x| x.as_f64()));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "p99 Tail Latency vs Database Size",
            &x_labels,
            &series,
            1000,
            500,
            "ns",
            "p99 tail latency as database grows from 1K to 5M entries (lower is better)",
            true,
        );
        let path = charts_dir.join("database-size-scaling.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);
    }

    // 5. Candlestick Percentile Charts
    // Candlestick for IPv4
    let ipv4_candle_rows = get_largest_lookup(aggregated, "ipv4", "random");
    if !ipv4_candle_rows.is_empty() {
        let mut candlesticks = Vec::new();
        for r in &ipv4_candle_rows {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let min_val = r.get("min_ns").and_then(|v| v.as_f64());
            let p50_val = r.get("p50_ns").and_then(|v| v.as_f64());
            let p95_val = r.get("p95_ns").and_then(|v| v.as_f64());
            let p99_val = r.get("p99_ns").and_then(|v| v.as_f64());
            let max_val = r.get("max_ns").and_then(|v| v.as_f64());

            if let (Some(min_ns), Some(p50_ns), Some(p95_ns), Some(p99_ns)) =
                (min_val, p50_val, p95_val, p99_val)
            {
                candlesticks.push(CandlestickStats {
                    label: impl_name.to_string(),
                    min_ns,
                    p50_ns,
                    p95_ns,
                    p99_ns,
                    max_ns: max_val.unwrap_or(p99_ns),
                    color: get_color(impl_name).to_string(),
                });
            }
        }
        if !candlesticks.is_empty() {
            candlesticks.sort_by(|a, b| a.p50_ns.partial_cmp(&b.p50_ns).unwrap());
            // Convert to (&str, CandlestickStats) pairs
            let items: Vec<_> = candlesticks
                .iter()
                .map(|s| (s.label.as_str(), s.clone()))
                .collect();
            let (_, svg) = svg_candlestick_chart(
                "Candlestick Percentile Rank — IPv4 Lookups",
                &items,
                1200,
                54,
                "Quantile spread: Min (lower wick) → p50/Median to p95 (body) → p99 Tail",
            );
            let path = charts_dir.join("candlestick-percentiles-ipv4.svg");
            let _ = fs::write(&path, svg);
            generated.push(path);
        }
    }

    // Candlestick for IPv6
    let ipv6_candle_rows = get_largest_lookup(aggregated, "ipv6", "random");
    if !ipv6_candle_rows.is_empty() {
        let mut candlesticks = Vec::new();
        for r in &ipv6_candle_rows {
            let impl_name = r
                .get("implementation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let min_val = r.get("min_ns").and_then(|v| v.as_f64());
            let p50_val = r.get("p50_ns").and_then(|v| v.as_f64());
            let p95_val = r.get("p95_ns").and_then(|v| v.as_f64());
            let p99_val = r.get("p99_ns").and_then(|v| v.as_f64());
            let max_val = r.get("max_ns").and_then(|v| v.as_f64());

            if let (Some(min_ns), Some(p50_ns), Some(p95_ns), Some(p99_ns)) =
                (min_val, p50_val, p95_val, p99_val)
            {
                candlesticks.push(CandlestickStats {
                    label: impl_name.to_string(),
                    min_ns,
                    p50_ns,
                    p95_ns,
                    p99_ns,
                    max_ns: max_val.unwrap_or(p99_ns),
                    color: get_color(impl_name).to_string(),
                });
            }
        }
        if !candlesticks.is_empty() {
            candlesticks.sort_by(|a, b| a.p50_ns.partial_cmp(&b.p50_ns).unwrap());
            // Convert to (&str, CandlestickStats) pairs
            let items: Vec<_> = candlesticks
                .iter()
                .map(|s| (s.label.as_str(), s.clone()))
                .collect();
            let (_, svg) = svg_candlestick_chart(
                "Candlestick Percentile Rank — IPv6 Lookups",
                &items,
                1200,
                54,
                "Quantile spread: Min (lower wick) → p50/Median to p95 (body) → p99 Tail",
            );
            let path = charts_dir.join("candlestick-percentiles-ipv6.svg");
            let _ = fs::write(&path, svg);
            generated.push(path);
        }
    }

    // 6. Thread Scaling
    let conc_rows = get_concurrent_rows(aggregated, "ipv4");
    if !conc_rows.is_empty() {
        let threads = [1, 4, 8, 16];
        let x_labels: Vec<String> = threads.iter().map(|t| format!("{t}T")).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &t in &threads {
                let v = conc_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("threads").and_then(|x| x.as_u64()) == Some(t as u64)
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "Throughput Scaling vs Thread Count",
            &x_labels,
            &series,
            1000,
            500,
            "M ops/s",
            "Throughput scaling efficiency across 1-16 threads (higher is better)",
            true,
        );
        let path = charts_dir.join("thread-scaling.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);
    }

    // 7. Database size scaling
    let scale_rows = get_scaling_rows(aggregated);
    if !scale_rows.is_empty() {
        let sizes = SCALING_DATABASE_SIZES;
        let x_labels: Vec<String> = sizes.iter().map(|s| fmt_size_tag(*s)).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = scale_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("database_size").and_then(|x| x.as_u64()) == Some(sz as u64)
                    })
                    .and_then(|r| r.get("p99_ns").and_then(|x| x.as_f64()));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "p99 Tail Latency vs Database Size",
            &x_labels,
            &series,
            1000,
            500,
            "ns",
            "p99 tail latency as database grows from 1K to 5M entries (lower is better)",
            true,
        );
        let path = charts_dir.join("database-size-scaling.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);

        // Database size p99 bar chart
        for &sz in SCALING_DATABASE_SIZES {
            let mut items = Vec::new();
            for &impl_name in IMPLEMENTATIONS {
                if let Some(v) = scale_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("database_size").and_then(|x| x.as_u64()) == Some(sz as u64)
                    })
                    .and_then(|r| r.get("p99_ns").and_then(|x| x.as_f64()))
                {
                    items.push((impl_name, v, fmt_duration(Some(v))));
                }
            }
            if !items.is_empty() {
                let (_, svg) = svg_bar_chart(
                    &format!("p99 Latency at {} entries", fmt_size_tag(sz)),
                    &items
                        .iter()
                        .map(|(a, b, c)| (*a, *b, c.as_str()))
                        .collect::<Vec<_>>(),
                    800,
                    40,
                    "ns",
                    &format!(
                        "p99 tail latency at {} entries (lower is better)",
                        fmt_size_tag(sz)
                    ),
                );
                let path = charts_dir.join(format!("database-size-p99-{sz}.svg"));
                let _ = fs::write(&path, &svg);
                generated.push(path);
                // Keep the existing link pointing at the largest measured database.
                let _ = fs::write(charts_dir.join("database-size-p99.svg"), svg);
            }
        }
    }

    // 8. Writer Benchmarks
    let writer_rows = get_writer_rows(aggregated);
    if !writer_rows.is_empty() {
        let sizes = WRITER_SIZES;
        let x_labels: Vec<String> = sizes.iter().map(|s| fmt_size_tag(*s)).collect();

        // Writer Throughput
        let mut throughput_series = Vec::new();
        for &impl_name in &["libmaxminddb-rs", "mmdbwriter"] {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                throughput_series.push((impl_name, vals));
            }
        }

        if !throughput_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Writer Insertion Throughput",
                &x_labels,
                &throughput_series,
                1000,
                500,
                "K ops/s",
                "Insertion rate across database sizes (higher is better)",
                true,
            );
            let path = charts_dir.join("writer-throughput.svg");
            let _ = fs::write(&path, svg);
            generated.push(path);
        }

        // Writer Build Time
        let mut build_time_series = Vec::new();
        for &impl_name in &["libmaxminddb-rs", "mmdbwriter"] {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("wall_time_ns").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                build_time_series.push((impl_name, vals));
            }
        }

        if !build_time_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Writer Total Time",
                &x_labels,
                &build_time_series,
                1000,
                500,
                "s",
                "Total wall clock time for MMDB generation (lower is better)",
                true,
            );
            let path = charts_dir.join("writer-build-time.svg");
            let _ = fs::write(&path, svg);
            generated.push(path);
        }

        // Writer Database Size
        let mut db_size_series = Vec::new();
        for &impl_name in &["libmaxminddb-rs", "mmdbwriter"] {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("database_size_bytes").and_then(|x| x.as_f64()))
                    .map(|v| v / (1024.0 * 1024.0));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                db_size_series.push((impl_name, vals));
            }
        }

        if !db_size_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Generated Database Size",
                &x_labels,
                &db_size_series,
                1000,
                500,
                "MiB",
                "Output MMDB file size (lower is better for same data)",
                true,
            );
            let path = charts_dir.join("writer-database-size.svg");
            let _ = fs::write(&path, svg);
            generated.push(path);
        }

        // Writer p99
        let mut p99_series = Vec::new();
        for &impl_name in &["libmaxminddb-rs", "mmdbwriter"] {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("p99_ns").and_then(|x| x.as_f64()));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                p99_series.push((impl_name, vals));
            }
        }

        if !p99_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Writer Insertion p99 Tail Latency",
                &x_labels,
                &p99_series,
                1000,
                500,
                "ns",
                "p99 insertion latency (lower is better)",
                true,
            );
            let path = charts_dir.join("writer-p99.svg");
            let _ = fs::write(&path, svg);
            generated.push(path);
        }

        // Writer Peak RSS
        let mut rss_series = Vec::new();
        for &impl_name in &["libmaxminddb-rs", "mmdbwriter"] {
            let mut vals = Vec::new();
            for &sz in sizes {
                let v = writer_rows
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && (r
                                .get("entries")
                                .or_else(|| r.get("database_size"))
                                .and_then(|x| x.as_u64())
                                == Some(sz as u64))
                    })
                    .and_then(|r| r.get("peak_rss_bytes").and_then(|x| x.as_f64()))
                    .map(|v| v / (1024.0 * 1024.0));
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                rss_series.push((impl_name, vals));
            }
        }

        if !rss_series.is_empty() {
            let (_, svg) = svg_line_chart(
                "Writer Peak Memory Usage (RSS)",
                &x_labels,
                &rss_series,
                1000,
                500,
                "MiB",
                "Peak RSS during MMDB generation across sizes (lower is better)",
                true,
            );
            let path_rss = charts_dir.join("writer-peak-rss.svg");
            let _ = fs::write(&path_rss, svg);
            generated.push(path_rss);
        }
    }

    // IPv6 Multi-threaded Throughput
    let conc_rows_v6 = get_concurrent_rows(aggregated, "ipv6");
    if !conc_rows_v6.is_empty() {
        let threads = [1, 4, 8, 16];
        let x_labels: Vec<String> = threads.iter().map(|t| format!("{t}T")).collect();
        let mut series = Vec::new();

        for &impl_name in IMPLEMENTATIONS {
            let mut vals = Vec::new();
            for &t in &threads {
                let v = conc_rows_v6
                    .iter()
                    .find(|r| {
                        r.get("implementation").and_then(|x| x.as_str()) == Some(impl_name)
                            && r.get("threads").and_then(|x| x.as_u64()) == Some(t as u64)
                    })
                    .and_then(|r| r.get("throughput_ops_s").and_then(|x| x.as_f64()))
                    .map(|v| v / 1_000_000.0);
                vals.push(v);
            }
            if vals.iter().any(|v| v.is_some()) {
                series.push((impl_name, vals));
            }
        }

        let (_, svg) = svg_line_chart(
            "Peak Concurrent Throughput — 16 Threads (IPv6)",
            &x_labels,
            &series,
            1000,
            500,
            "M ops/s",
            "IPv6 throughput by thread count, 1 to 16 threads (higher is better)",
            true,
        );
        let path = charts_dir.join("concurrent-throughput-ipv6.svg");
        let _ = fs::write(&path, svg);
        generated.push(path);
    }

    // Single-Threaded throughput charts for IPv4 & IPv6 patterns
    for &fam in &["ipv4", "ipv6"] {
        for &pat in &["random", "sequential", "hot", "absent"] {
            let rows = get_largest_lookup(aggregated, fam, pat);
            if !rows.is_empty() {
                let mut items = Vec::new();
                for r in &rows {
                    let impl_name = r
                        .get("implementation")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let unsupported = r
                        .get("unsupported")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if r.get("failed").and_then(|v| v.as_bool()).unwrap_or(false) {
                        items.push((impl_name, 0.0, "⚠️ failed".to_string()));
                    } else if unsupported {
                        items.push((impl_name, 0.0, "⚠️ unsupported".to_string()));
                    } else if let Some(tp) = r.get("throughput_ops_s").and_then(|v| v.as_f64()) {
                        items.push((impl_name, tp, fmt_rate(Some(tp))));
                    }
                }
                if items.iter().any(|x| x.1 > 0.0) {
                    let family_title = if fam == "ipv4" { "IPv4" } else { "IPv6" };
                    let pattern_title = match pat {
                        "random" => "Random Lookups",
                        "absent" => "Absent Keys",
                        "sequential" => "Sequential Lookups",
                        _ => "Hot Lookups",
                    };
                    let title = format!("{family_title} Throughput — {pattern_title} (1M)");
                    let sub = format!(
                        "Single-threaded operations/sec on {} workload (higher is better)",
                        pat
                    );
                    let (_, svg) = svg_bar_chart(
                        &title,
                        &items
                            .iter()
                            .map(|(a, b, c)| (*a, *b, c.as_str()))
                            .collect::<Vec<_>>(),
                        1000,
                        48,
                        "max",
                        &sub,
                    );
                    let file_name = format!("throughput-{}-{}.svg", fam, pat);
                    let path = charts_dir.join(file_name);
                    let _ = fs::write(&path, svg);
                    generated.push(path);
                }
            }
        }
    }

    // Resident memory (RSS) vs database size for the four memory libraries.
    generated.extend(generate_memory_svg_charts(aggregated, charts_dir));

    generated
}
