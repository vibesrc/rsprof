use super::app::{App, ChartType, Focus};
use crate::storage::CpuEntry;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table,
    },
    Frame,
};

pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header (single line, no border)
            Constraint::Min(10),   // Main content
            Constraint::Length(1), // Footer (single line, no border)
        ])
        .split(frame.area());

    render_header(frame, app, chunks[0]);
    render_main_content(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let elapsed = app.elapsed();
    let hours = elapsed.as_secs() / 3600;
    let minutes = (elapsed.as_secs() % 3600) / 60;
    let seconds = elapsed.as_secs() % 60;

    let header = if app.is_static() {
        // Static/view mode header
        let file_name = app.file_name().unwrap_or("profile");
        Line::from(vec![
            Span::styled("rsprof", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(" VIEW ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(format!(
                " {} │ {:02}:{:02}:{:02} │ {} samples",
                file_name, hours, minutes, seconds, app.total_samples()
            )),
        ])
    } else {
        // Live recording mode header
        let status = if app.is_paused() {
            Span::styled(" PAUSED ", Style::default().bg(Color::Yellow).fg(Color::Black))
        } else {
            Span::styled(" RECORDING ", Style::default().bg(Color::Green).fg(Color::Black))
        };

        Line::from(vec![
            Span::styled("rsprof", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            status,
            Span::raw(format!(
                " {:02}:{:02}:{:02} │ {} samples",
                hours, minutes, seconds, app.total_samples()
            )),
        ])
    };

    let paragraph = Paragraph::new(header);
    frame.render_widget(paragraph, area);
}

fn render_main_content(frame: &mut Frame, app: &mut App, area: Rect) {
    // Split: left table (60%) | right chart (40%)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60),
            Constraint::Percentage(40),
        ])
        .split(area);

    // Store areas for mouse click detection
    app.set_table_area(chunks[0]);
    app.set_chart_area(chunks[1]);

    let entries = app.entries();
    let elapsed_secs = app.elapsed_secs();
    render_cpu_table(frame, app, entries, chunks[0]);
    render_line_chart(frame, app, elapsed_secs, chunks[1]);
}

fn render_cpu_table(frame: &mut Frame, app: &App, entries: &[CpuEntry], area: Rect) {
    let border_color = if app.focus == Focus::Table {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .title(" Top CPU ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let header_cells = ["Total", "Now", "Location", "Function"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let selected = app.selected_row().min(entries.len().saturating_sub(1));
    let visible_height = area.height.saturating_sub(3) as usize;

    // Use scroll offset from app (can be different from selection-based scroll)
    let max_scroll = entries.len().saturating_sub(visible_height.max(1));
    let scroll_offset = app.scroll_offset().min(max_scroll);

    let rows: Vec<Row> = entries
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height.max(1))
        .map(|(i, entry)| {
            let total_str = format!("{:5.1}%", entry.total_percent);
            let instant_str = format!("{:5.1}%", entry.instant_percent);
            let location = format_location(&entry.file, entry.line);
            let function = format_function(&entry.function);

            let style = if i == selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(total_str).style(Style::default().fg(color_for_percent(entry.total_percent))),
                Cell::from(instant_str).style(Style::default().fg(color_for_percent(entry.instant_percent))),
                Cell::from(location),
                Cell::from(function),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Percentage(30),
        Constraint::Percentage(50),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block);

    frame.render_widget(table, area);

    // Render scrollbar if there are more entries than visible
    if entries.len() > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");

        let mut scrollbar_state = ScrollbarState::new(entries.len())
            .position(scroll_offset);

        // Render scrollbar in the table area (inside the border)
        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn render_line_chart(frame: &mut Frame, app: &mut App, elapsed_secs: f64, area: Rect) {
    let border_color = if app.focus == Focus::Chart {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    // Get selected function name for title (strip hash suffix and simplify)
    let base_title = if let Some(func) = app.selected_func() {
        let clean = strip_hash_suffix(func);
        let short = clean.split("::").last().unwrap_or(&clean);
        short.to_string()
    } else {
        "CPU%".to_string()
    };

    // Get visible time range from chart state
    let (x_start, x_end) = app.chart_state.visible_range(elapsed_secs);

    // Get zoom label and chart type before mutable borrow
    let zoom_label = app.chart_state.zoom_label();
    let chart_type = app.chart_state.chart_type;
    let chart_type_label = match chart_type {
        ChartType::Line => "line",
        ChartType::Bar => "bar",
    };
    let title = format!(" {} [{}] ({}) ", base_title, zoom_label, chart_type_label);

    // Calculate chart inner width for aggregation
    // Chart layout: borders(2) + y-axis title(2) + y-axis labels(5 for "100%") + spacing(1) = ~10
    let chart_inner_width = area.width.saturating_sub(10).max(1) as usize;

    // Query data aggregated at DB level (with caching and prefetch)
    // Clone to release the borrow
    let chart_data: Vec<(f64, f64)> = app.query_chart_data(x_start, x_end, chart_inner_width).to_vec();

    let block = Block::default()
        .title(title.clone())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    if chart_data.is_empty() {
        let msg = Paragraph::new(" Collecting data...")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    // Filter data to visible range (cache may have prefetched extra data)
    let visible_data: Vec<(f64, f64)> = chart_data
        .iter()
        .filter(|(t, _)| *t >= x_start && *t <= x_end)
        .copied()
        .collect();

    // Calculate y bounds from visible data
    let (y_min, y_max) = if visible_data.is_empty() {
        (0.0, 100.0)
    } else {
        let min_y = visible_data.iter().map(|(_, y)| *y).fold(f64::MAX, f64::min);
        let max_y = visible_data.iter().map(|(_, y)| *y).fold(0.0f64, f64::max);
        let range = (max_y - min_y).max(1.0);
        let padding = range * 0.1;
        (
            ((min_y - padding).max(0.0) / 5.0).floor() * 5.0,
            ((max_y + padding) / 5.0).ceil() * 5.0,
        )
    };

    let (marker, graph_type) = match chart_type {
        ChartType::Line => (symbols::Marker::Braille, GraphType::Line),
        ChartType::Bar => (symbols::Marker::HalfBlock, GraphType::Bar),
    };

    let datasets = vec![
        Dataset::default()
            .marker(marker)
            .graph_type(graph_type)
            .style(Style::default().fg(Color::Green))
            .data(&visible_data),
    ];

    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([x_start, x_end]),
        )
        .y_axis(
            Axis::default()
                .title("%")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([y_min, y_max])
                .labels(vec![
                    Span::raw(format!("{:.0}%", y_min)),
                    Span::raw(format!("{:.0}%", (y_min + y_max) / 2.0)),
                    Span::raw(format!("{:.0}%", y_max)),
                ]),
        );

    frame.render_widget(chart, area);
}

/// Strip the hash suffix from Rust function names (e.g., "foo::h1234abcd" -> "foo")
fn strip_hash_suffix(name: &str) -> String {
    if let Some(idx) = name.rfind("::h") {
        let suffix = &name[idx + 3..];
        if suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
            return name[..idx].to_string();
        }
    }
    name.to_string()
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" q ", Style::default().bg(Color::DarkGray)),
        Span::raw(" quit "),
    ];

    // Only show pause in live mode
    if !app.is_static() {
        spans.push(Span::styled(" p ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" pause "));
    }

    spans.push(Span::styled(" Tab ", Style::default().bg(Color::DarkGray)));
    spans.push(Span::raw(" focus "));

    // Show context-sensitive help based on focus
    if app.focus == Focus::Table {
        spans.push(Span::styled(" j/k ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" nav "));
        spans.push(Span::styled(" ^d/u ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" page "));
        spans.push(Span::styled(" g/G ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" top/end "));
    } else {
        spans.push(Span::styled(" h/l ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" pan "));
        spans.push(Span::styled(" +/- ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" zoom "));
        spans.push(Span::styled(" ␣ ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" follow "));
        spans.push(Span::styled(" b ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" line/bar "));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, area);
}

fn color_for_percent(pct: f64) -> Color {
    if pct >= 20.0 {
        Color::Red
    } else if pct >= 10.0 {
        Color::Yellow
    } else if pct >= 5.0 {
        Color::Green
    } else {
        Color::White
    }
}

fn format_location(file: &str, line: u32) -> String {
    let simplified = simplify_path(file);
    if line > 0 {
        format!("{}:{}", simplified, line)
    } else {
        simplified
    }
}

fn simplify_path(path: &str) -> String {
    if path.starts_with('[') {
        return path.to_string();
    }
    if path.contains("/rust/library/") || path.contains("/rustc/") {
        if let Some(filename) = path.rsplit('/').next() {
            return format!("<std>/{}", filename);
        }
    }
    if path.contains("/.cargo/") {
        if let Some(idx) = path.find("/src/") {
            let before_src = &path[..idx];
            if let Some(crate_start) = before_src.rfind('/') {
                let crate_name = &before_src[crate_start + 1..];
                let after_src = &path[idx + 5..];
                return format!("<{}>/{}", crate_name, after_src);
            }
        }
    }
    if let Some(idx) = path.find("/src/") {
        return path[idx + 1..].to_string();
    }
    if let Some(idx) = path.find("/examples/") {
        return path[idx + 1..].to_string();
    }
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn format_function(func: &str) -> String {
    let mut result = func.to_string();

    // Remove hash suffix
    if let Some(idx) = result.rfind("::h") {
        let suffix = &result[idx + 3..];
        if suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
            result = result[..idx].to_string();
        }
    }

    // Simplify common prefixes
    let prefixes = [
        ("core::slice::sort::", "sort::"),
        ("core::ptr::", "ptr::"),
        ("core::fmt::", "fmt::"),
        ("core::iter::", "iter::"),
        ("alloc::vec::", "Vec::"),
        ("alloc::string::", "String::"),
        ("hashbrown::raw::", "hashbrown::"),
        ("std::collections::hash_map::", "HashMap::"),
    ];

    for (prefix, replacement) in prefixes {
        if result.starts_with(prefix) {
            result = format!("{}{}", replacement, &result[prefix.len()..]);
            break;
        }
    }

    result
}
