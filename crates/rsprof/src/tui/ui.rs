use super::app::{App, ChartType, Focus, SortColumn, TableSort, ViewMode};
use crate::storage::{CpuEntry, HeapEntry};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span, Text},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table,
    },
};
use std::collections::{HashMap, VecDeque};

/// Unified table row data - used by all table views
struct TableRow {
    /// Primary metric value (formatted string, e.g., "12.3%" or "1.2MB")
    total: String,
    /// Secondary/live metric value
    live: String,
    /// Function name (already formatted)
    function: String,
    /// Location string (file:line)
    location: String,
    /// Sparkline data points (values for rendering)
    sparkline_data: Vec<i64>,
    /// Color for the total column
    total_color: Color,
    /// Color for the live column
    live_color: Color,
}

/// Convert CPU entries to unified table rows
fn cpu_to_table_rows(
    entries: &[CpuEntry],
    sparklines: &HashMap<i64, VecDeque<i64>>,
) -> Vec<TableRow> {
    entries
        .iter()
        .map(|e| {
            // Use location sparkline if available, otherwise generate from current values
            let sparkline_data: Vec<i64> = sparklines
                .get(&e.location_id)
                .map(|v| v.iter().copied().collect())
                .unwrap_or_else(|| {
                    // Generate simple sparkline from total/instant as percentages * 1000
                    vec![
                        (e.total_percent * 1000.0) as i64,
                        (e.instant_percent * 1000.0) as i64,
                    ]
                });

            TableRow {
                total: format!("{:5.1}%", e.total_percent),
                live: format!("{:5.1}%", e.instant_percent),
                function: format_function(&e.function),
                location: format_location(&e.file, e.line),
                sparkline_data,
                total_color: color_for_percent(e.total_percent),
                live_color: color_for_percent(e.instant_percent),
            }
        })
        .collect()
}

/// Convert Heap entries to unified table rows
fn heap_to_table_rows(
    entries: &[HeapEntry],
    sparklines: &HashMap<i64, VecDeque<i64>>,
) -> Vec<TableRow> {
    entries
        .iter()
        .map(|e| {
            let sparkline_data: Vec<i64> = sparklines
                .get(&e.location_id)
                .map(|v| v.iter().copied().collect())
                .unwrap_or_else(|| vec![e.total_alloc_bytes, e.live_bytes]);

            TableRow {
                total: format_bytes(e.total_alloc_bytes),
                live: format_bytes(e.live_bytes),
                function: format_function(&e.function),
                location: format_location(&e.file, e.line),
                sparkline_data,
                total_color: color_for_bytes(e.total_alloc_bytes),
                live_color: color_for_bytes(e.live_bytes),
            }
        })
        .collect()
}

/// Render a unified table with the standard layout
fn render_unified_table(
    frame: &mut Frame,
    title: &str,
    rows: &[TableRow],
    selected: usize,
    scroll_offset: usize,
    focus: Focus,
    sort: TableSort,
    area: Rect,
) {
    let border_color = if focus == Focus::Table {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    if rows.is_empty() {
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "No data...",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    let header_labels = [
        header_label("Total", SortColumn::Total, sort),
        header_label("Live", SortColumn::Live, sort),
        header_label("Function", SortColumn::Function, sort),
        header_label("Location", SortColumn::Location, sort),
        header_label("Trend", SortColumn::Trend, sort),
    ];
    let header_cells = header_labels.iter().map(|h| {
        Cell::from(h.as_str()).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1);

    let visible_height = area.height.saturating_sub(3) as usize;
    let max_scroll = rows.len().saturating_sub(visible_height.max(1));
    let scroll_offset = scroll_offset.min(max_scroll);
    let selected = selected.min(rows.len().saturating_sub(1));

    // Find global max for sparkline heatmap coloring
    let global_max = rows
        .iter()
        .flat_map(|r| r.sparkline_data.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1);

    let table_rows: Vec<Row> = rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height.max(1))
        .map(|(i, row)| {
            // Sparkline with per-character coloring
            let sparkline_line = render_sparkline(&row.sparkline_data, 12, global_max);

            let style = if i == selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(row.total.clone()).style(Style::default().fg(row.total_color)),
                Cell::from(row.live.clone()).style(Style::default().fg(row.live_color)),
                Cell::from(row.function.clone()),
                Cell::from(row.location.clone()),
                Cell::from(sparkline_line),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(8),  // Total (fixed)
        Constraint::Length(8),  // Live (fixed)
        Constraint::Fill(1),    // Function (expand)
        Constraint::Fill(1),    // Location (expand)
        Constraint::Length(14), // Trend (fixed, 12 chars + padding)
    ];

    let table = Table::new(table_rows, widths).header(header).block(block);

    frame.render_widget(table, area);

    // Render scrollbar if there are more entries than visible
    if rows.len() > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .track_symbol(Some("│"))
            .thumb_symbol("█");

        let mut scrollbar_state = ScrollbarState::new(rows.len()).position(scroll_offset);

        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn header_label(label: &str, column: SortColumn, sort: TableSort) -> String {
    if sort.column != column {
        return label.to_string();
    }
    let indicator = if sort.descending { "v" } else { "^" };
    format!("{} {}", label, indicator)
}

/// Render sparkline from data points with per-character coloring
/// Data is expected in chronological order (oldest first, newest last)
/// New data appears on the RIGHT, old data shifts LEFT
fn render_sparkline(values: &[i64], width: usize, global_max: i64) -> Text<'static> {
    if values.is_empty() {
        return Text::styled("·".repeat(width), Style::default().fg(Color::DarkGray));
    }

    let min_val = *values.iter().min().unwrap_or(&0);
    let max_val = *values.iter().max().unwrap_or(&0);
    let range = (max_val - min_val) as f64;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(width);

    // Calculate how many empty slots on the left (for data we don't have yet)
    let data_points = values.len().min(width);
    let empty_slots = width.saturating_sub(data_points);

    // Fill empty slots on the LEFT with dots (no data for that time period)
    for _ in 0..empty_slots {
        spans.push(Span::styled(
            "·".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Sample values if we have more than width, otherwise use all
    let step = if values.len() > width {
        values.len() as f64 / width as f64
    } else {
        1.0
    };

    // Render actual data points (oldest on left, newest on right)
    for i in 0..data_points {
        let idx = if values.len() > width {
            // Sample from the data
            (i as f64 * step) as usize
        } else {
            // Use values directly, taking from the end if we have fewer than width
            values.len().saturating_sub(data_points) + i
        };

        let val = values.get(idx).copied().unwrap_or(0);

        // Zero value = no data, show dot
        if val == 0 {
            spans.push(Span::styled(
                "·".to_string(),
                Style::default().fg(Color::DarkGray),
            ));
            continue;
        }

        let char_idx = if range == 0.0 || global_max == 0 {
            // All non-zero values are the same, use middle height
            3
        } else {
            // Normalize against global max for consistent scaling across rows
            let normalized = (val as f64 / global_max as f64 * 7.0).round() as usize;
            normalized.min(7)
        };

        // Color based on character height (visual representation)
        // Higher bars = hotter colors
        let color = match char_idx {
            7 => Color::Red,
            6 => Color::LightRed,
            5 => Color::Yellow,
            4 => Color::LightYellow,
            3 => Color::Green,
            2 => Color::LightGreen,
            1 => Color::Cyan,
            _ => Color::DarkGray,
        };

        spans.push(Span::styled(
            SPARKLINE_CHARS[char_idx].to_string(),
            Style::default().fg(color),
        ));
    }

    Text::from(Line::from(spans))
}

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
    // Split header: left (status) | right (tabs)
    let chunks = Layout::horizontal([
        Constraint::Min(40),
        Constraint::Length(15), // "[CPU] [Memory]"
    ])
    .split(area);

    render_header_status(frame, app, chunks[0]);
    render_header_tabs(frame, app, chunks[1]);
}

fn render_header_status(frame: &mut Frame, app: &App, area: Rect) {
    let elapsed = app.elapsed();
    let hours = elapsed.as_secs() / 3600;
    let minutes = (elapsed.as_secs() % 3600) / 60;
    let seconds = elapsed.as_secs() % 60;

    let header = if app.is_static() {
        // Static/view mode header
        let file_name = app.file_name().unwrap_or("profile");
        Line::from(vec![
            Span::styled(
                "rsprof",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(" VIEW ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(format!(
                " {} │ {:02}:{:02}:{:02} │ {} samples",
                file_name,
                hours,
                minutes,
                seconds,
                app.total_samples()
            )),
        ])
    } else {
        // Live recording mode header
        let status = if app.is_paused() {
            Span::styled(
                " PAUSED ",
                Style::default().bg(Color::Yellow).fg(Color::Black),
            )
        } else {
            Span::styled(
                " RECORDING ",
                Style::default().bg(Color::Green).fg(Color::Black),
            )
        };

        Line::from(vec![
            Span::styled(
                "rsprof",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            status,
            Span::raw(format!(
                " {:02}:{:02}:{:02} │ {} samples",
                hours,
                minutes,
                seconds,
                app.total_samples()
            )),
        ])
    };

    let paragraph = Paragraph::new(header);
    frame.render_widget(paragraph, area);
}

fn render_header_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let active_style = Style::default().bg(Color::Cyan).fg(Color::Black);
    let inactive_style = Style::default().fg(Color::DarkGray);

    let cpu_style = if app.view_mode == ViewMode::Cpu {
        active_style
    } else {
        inactive_style
    };
    let mem_style = if app.view_mode == ViewMode::Memory {
        active_style
    } else {
        inactive_style
    };

    let tabs = Line::from(vec![
        Span::styled("[CPU]", cpu_style),
        Span::raw(" "),
        Span::styled("[Memory]", mem_style),
    ]);

    let paragraph = Paragraph::new(tabs);
    frame.render_widget(paragraph, area);
}

fn render_main_content(frame: &mut Frame, app: &mut App, area: Rect) {
    let elapsed_secs = app.elapsed_secs();
    let view_mode = app.view_mode;
    let chart_visible = app.chart_visible;
    let selected = app.selected_row();
    let scroll_offset = app.scroll_offset();
    let focus = app.focus;
    let sort = app.active_sort();

    // Prepare table data based on view mode (use appropriate sparklines)
    let (title, rows) = match view_mode {
        ViewMode::Cpu => {
            let entries = app.entries();
            let sparklines = app.cpu_sparklines().clone();
            ("Top CPU", cpu_to_table_rows(entries, &sparklines))
        }
        ViewMode::Memory => {
            let entries = app.heap_entries();
            let sparklines = app.heap_sparklines().clone();
            ("Top Memory", heap_to_table_rows(entries, &sparklines))
        }
    };

    if chart_visible {
        // Split: left table (60%) | right chart (40%)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        // Store areas for mouse click detection
        app.set_table_area(chunks[0]);
        app.set_chart_area(chunks[1]);

        // Render unified table
        render_unified_table(frame, title, &rows, selected, scroll_offset, focus, sort, chunks[0]);

        // Render appropriate chart
        match view_mode {
            ViewMode::Cpu => render_line_chart(frame, app, elapsed_secs, chunks[1]),
            ViewMode::Memory => render_memory_chart(frame, app, elapsed_secs, chunks[1]),
        }
    } else {
        // Full-width table with sparklines (no chart)
        app.set_table_area(area);
        app.set_chart_area(Rect::default());

        render_unified_table(frame, title, &rows, selected, scroll_offset, focus, sort, area);
}
}

fn render_memory_chart(frame: &mut Frame, app: &mut App, elapsed_secs: f64, area: Rect) {
    let border_color = if app.focus == Focus::Chart {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    // Get selected function name for title
    let base_title = if let Some(func) = app.selected_heap_func() {
        let clean = strip_hash_suffix(func);
        let short = clean.split("::").last().unwrap_or(&clean);
        short.to_string()
    } else {
        "Memory".to_string()
    };

    // Get visible time range from chart state
    let (x_start, x_end) = app.chart_state.visible_range(elapsed_secs);

    // Get zoom label and chart type
    let zoom_label = app.chart_state.zoom_label();
    let chart_type = app.chart_state.chart_type;
    let chart_type_label = match chart_type {
        ChartType::Line => "line",
        ChartType::Bar => "bar",
    };
    let title = format!(" {} [{}] ({}) ", base_title, zoom_label, chart_type_label);

    // Calculate chart inner width for aggregation
    let chart_inner_width = area.width.saturating_sub(12).max(1) as usize;

    // Query heap data aggregated at DB level
    let chart_data: Vec<(f64, f64)> = app
        .query_heap_chart_data(x_start, x_end, chart_inner_width)
        .to_vec();

    let block = Block::default()
        .title(title.clone())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    if chart_data.is_empty() {
        let msg = Paragraph::new(" No memory data...")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    // Filter data to visible range
    let visible_data: Vec<(f64, f64)> = chart_data
        .iter()
        .filter(|(t, _)| *t >= x_start && *t <= x_end)
        .copied()
        .collect();

    // Calculate y bounds from visible data (bytes)
    let (y_min, y_max) = if visible_data.is_empty() {
        (0.0, 1000000.0) // Default to 1MB
    } else {
        let min_y = visible_data
            .iter()
            .map(|(_, y)| *y)
            .fold(f64::MAX, f64::min);
        let max_y = visible_data.iter().map(|(_, y)| *y).fold(0.0f64, f64::max);
        let range = (max_y - min_y).max(1.0);
        let padding = range * 0.1;
        ((min_y - padding).max(0.0), max_y + padding)
    };

    let (marker, graph_type) = match chart_type {
        ChartType::Line => (symbols::Marker::Braille, GraphType::Line),
        ChartType::Bar => (symbols::Marker::HalfBlock, GraphType::Bar),
    };

    let datasets = vec![
        Dataset::default()
            .marker(marker)
            .graph_type(graph_type)
            .style(Style::default().fg(Color::Magenta))
            .data(&visible_data),
    ];

    // Generate x-axis labels
    let x_labels = generate_time_labels(x_start, x_end);

    // Generate y-axis labels with byte formatting
    let y_labels = vec![
        Span::raw(format_bytes_short(y_min as i64)),
        Span::raw(format_bytes_short(((y_min + y_max) / 2.0) as i64)),
        Span::raw(format_bytes_short(y_max as i64)),
    ];

    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([x_start, x_end])
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .title("B")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([y_min, y_max])
                .labels(y_labels),
        );

    frame.render_widget(chart, area);
}

/// Format bytes for y-axis labels (short form)
fn format_bytes_short(bytes: i64) -> String {
    let abs_bytes = bytes.abs() as f64;
    if abs_bytes >= 1_073_741_824.0 {
        format!("{:.0}G", abs_bytes / 1_073_741_824.0)
    } else if abs_bytes >= 1_048_576.0 {
        format!("{:.0}M", abs_bytes / 1_048_576.0)
    } else if abs_bytes >= 1024.0 {
        format!("{:.0}K", abs_bytes / 1024.0)
    } else {
        format!("{}", bytes.abs())
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
    let chart_data: Vec<(f64, f64)> = app
        .query_chart_data(x_start, x_end, chart_inner_width)
        .to_vec();

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
        let min_y = visible_data
            .iter()
            .map(|(_, y)| *y)
            .fold(f64::MAX, f64::min);
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

    // Generate x-axis labels based on visible range
    let x_labels = generate_time_labels(x_start, x_end);

    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([x_start, x_end])
                .labels(x_labels),
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

/// Generate x-axis time labels: start, middle, end
/// Adapts unit (seconds, minutes, hours) based on zoom level
fn generate_time_labels(start: f64, end: f64) -> Vec<Span<'static>> {
    let mid = (start + end) / 2.0;

    vec![
        Span::raw(format_time(start.max(0.0))),
        Span::raw(format_time(mid.max(0.0))),
        Span::raw(format_time(end)),
    ]
}

/// Format time value with appropriate unit
fn format_time(secs: f64) -> String {
    if secs >= 3600.0 {
        let h = (secs / 3600.0) as i64;
        let m = ((secs % 3600.0) / 60.0) as i64;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h{}m", h, m)
        }
    } else if secs >= 60.0 {
        let m = (secs / 60.0) as i64;
        let s = (secs % 60.0) as i64;
        if s == 0 {
            format!("{}m", m)
        } else {
            format!("{}m{}s", m, s)
        }
    } else {
        format!("{}s", secs as i64)
    }
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

/// Format bytes into human-readable units (B, KB, MB, GB, TB)
fn format_bytes(bytes: i64) -> String {
    let abs_bytes = bytes.abs() as f64;
    let sign = if bytes < 0 { "-" } else { "" };

    if abs_bytes >= 1_099_511_627_776.0 {
        format!("{}{:.1}TB", sign, abs_bytes / 1_099_511_627_776.0)
    } else if abs_bytes >= 1_073_741_824.0 {
        format!("{}{:.1}GB", sign, abs_bytes / 1_073_741_824.0)
    } else if abs_bytes >= 1_048_576.0 {
        format!("{}{:.1}MB", sign, abs_bytes / 1_048_576.0)
    } else if abs_bytes >= 1024.0 {
        format!("{}{:.1}KB", sign, abs_bytes / 1024.0)
    } else {
        format!("{}{}B", sign, bytes.abs())
    }
}

/// Color for memory amount based on size
fn color_for_bytes(bytes: i64) -> Color {
    if bytes >= 100_000_000 {
        // 100MB+
        Color::Red
    } else if bytes >= 10_000_000 {
        // 10MB+
        Color::Yellow
    } else if bytes >= 1_000_000 {
        // 1MB+
        Color::Green
    } else {
        Color::White
    }
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

    // View mode hint
    spans.push(Span::styled(" m ", Style::default().bg(Color::DarkGray)));
    spans.push(Span::raw(" mode "));

    // Chart toggle - show/hide
    let chart_label = if app.chart_visible {
        "hide chart"
    } else {
        "show chart"
    };
    spans.push(Span::styled(" c ", Style::default().bg(Color::DarkGray)));
    spans.push(Span::raw(format!(" {} ", chart_label)));

    // Context-sensitive help based on chart visibility and focus
    if app.chart_visible {
        spans.push(Span::styled(" Tab ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" focus "));

        if app.focus == Focus::Table {
            spans.push(Span::styled(" j/k ", Style::default().bg(Color::DarkGray)));
            spans.push(Span::raw(" nav "));
        } else {
            spans.push(Span::styled(" h/l ", Style::default().bg(Color::DarkGray)));
            spans.push(Span::raw(" pan "));
            spans.push(Span::styled(" +/- ", Style::default().bg(Color::DarkGray)));
            spans.push(Span::raw(" zoom "));
        }
    } else {
        // Table-only mode
        spans.push(Span::styled(" j/k ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" nav "));
        spans.push(Span::styled(" ^d/u ", Style::default().bg(Color::DarkGray)));
        spans.push(Span::raw(" page "));
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
    if (path.contains("/rust/library/") || path.contains("/rustc/"))
        && let Some(filename) = path.rsplit('/').next()
    {
        return format!("<std>/{}", filename);
    }
    if path.contains("/.cargo/")
        && let Some(idx) = path.find("/src/")
    {
        let before_src = &path[..idx];
        if let Some(crate_start) = before_src.rfind('/') {
            let crate_name = &before_src[crate_start + 1..];
            let after_src = &path[idx + 5..];
            return format!("<{}>/{}", crate_name, after_src);
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

    // Simplify trait impls: <Type as Trait>::method -> Type::method
    if result.starts_with('<')
        && let Some(as_pos) = result.find(" as ")
        && let Some(gt_pos) = result.find(">::")
    {
        let impl_type = &result[1..as_pos];
        let method = &result[gt_pos + 3..];
        let type_short = simplify_type_path(impl_type);
        result = format!("{}::{}", type_short, method);
    }

    // Simplify common prefixes
    let prefixes = [
        ("core::slice::sort::", "sort::"),
        ("core::ptr::", "ptr::"),
        ("core::fmt::", "fmt::"),
        ("core::iter::", "iter::"),
        ("core::hash::", "hash::"),
        ("core::str::", "str::"),
        ("core::num::", "num::"),
        ("alloc::vec::", "Vec::"),
        ("alloc::string::", "String::"),
        ("alloc::alloc::", "alloc::"),
        ("hashbrown::raw::", "hashbrown::"),
        ("std::collections::hash_map::", "HashMap::"),
    ];

    for (prefix, replacement) in prefixes {
        if result.starts_with(prefix) {
            result = format!("{}{}", replacement, &result[prefix.len()..]);
            break;
        }
    }

    // Remove complex generic parameters
    while let (Some(start), Some(end)) = (result.find('<'), result.rfind('>')) {
        if start < end {
            let generic = &result[start..=end];
            if generic.len() > 20 || generic.contains("::") {
                result = format!("{}<_>{}", &result[..start], &result[end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    result
}

/// Simplify a type path to module::Type format
fn simplify_type_path(path: &str) -> String {
    let parts: Vec<&str> = path.split("::").collect();
    if parts.len() >= 2 {
        format!("{}::{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        path.to_string()
    }
}

/// Unicode block characters for sparklines (8 levels from empty to full)
const SPARKLINE_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
