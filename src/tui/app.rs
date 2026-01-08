use crate::cpu::CpuSampler;
use crate::error::Result;
use crate::heap::HeapSampler;
use crate::storage::{CpuEntry, HeapEntry, Storage, query_cpu_timeseries_aggregated};
use crate::symbols::SymbolResolver;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, Terminal};
use rusqlite::Connection;
use std::io::{self, stdout};
use std::path::Path;
use std::time::{Duration, Instant};

/// Cache for chart data with prefetch window
#[derive(Default)]
struct ChartDataCache {
    location_id: Option<i64>,
    /// Cached time range (wider than visible for prefetch)
    cache_start_secs: f64,
    cache_end_secs: f64,
    /// Number of points per second in cached data
    points_per_sec: f64,
    /// Cached data points
    data: Vec<(f64, f64)>,
}

use super::ui;

/// Find the first "user" frame in a stack trace (not allocator internals)
fn find_user_frame(stack: &[u64], resolver: &SymbolResolver) -> crate::symbols::Location {
    let skip_function_patterns = [
        "__rust_alloc", "__rust_dealloc", "__rust_realloc",
        "alloc::alloc::", "alloc::raw_vec::", "alloc::vec::",
        "alloc::string::", "alloc::collections::", "<alloc::",
        "hashbrown::", "std::collections::hash",
        "core::ptr::", "core::slice::", "core::iter::", "<core::",
        "core::ops::function::",
        "_Unwind_", "__cxa_", "_fini", "_init",
        "addr2line::", "gimli::", "object::", "miniz_oxide::",
        "sort::shared::smallsort::",
    ];

    fn is_internal_file(file: &str) -> bool {
        file.is_empty()
            || file.starts_with('[')
            || file.starts_with('<')
            || file.contains("/rustc/")
            || file.contains("/.cargo/registry/")
            || file.contains("/rust/library/")
    }

    for &addr in stack {
        let loc = resolver.resolve(addr);
        if is_internal_file(&loc.file) {
            continue;
        }
        let is_internal_fn = skip_function_patterns.iter().any(|p| loc.function.contains(p));
        if !is_internal_fn && (loc.file.contains("/src/") || loc.file.contains("/examples/")) {
            return loc;
        }
    }

    for &addr in stack {
        let loc = resolver.resolve(addr);
        if is_internal_file(&loc.file) {
            continue;
        }
        let is_internal_fn = skip_function_patterns.iter().any(|p| loc.function.contains(p));
        if !is_internal_fn {
            return loc;
        }
    }

    for &addr in stack {
        let loc = resolver.resolve(addr);
        let is_internal_fn = skip_function_patterns.iter().any(|p| loc.function.contains(p));
        if !is_internal_fn && !loc.function.is_empty() && loc.function != "[unknown]" {
            return loc;
        }
    }

    if !stack.is_empty() {
        return resolver.resolve(stack[0]);
    }

    resolver.resolve(0)
}

/// Focus state for keyboard navigation
#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Table,
    Chart,
}

/// Chart visualization type
#[derive(Clone, Copy, PartialEq, Default)]
pub enum ChartType {
    #[default]
    Line,
    Bar,
}

/// View mode for switching between CPU, Memory, and Both views
#[derive(Clone, Copy, PartialEq, Default)]
pub enum ViewMode {
    #[default]
    Cpu,
    Memory,
    Both,
}

/// Sort column for "Both" view
#[derive(Clone, Copy, PartialEq, Default)]
pub enum SortColumn {
    #[default]
    Cpu,
    Memory,
}

/// Fixed zoom levels with corresponding aggregation bucket sizes
/// (window_secs, bucket_secs) - bucket is None if no aggregation needed
const ZOOM_LEVELS: &[(f64, Option<f64>)] = &[
    (5.0, None),           // 5s  - no aggregation
    (10.0, None),          // 10s - no aggregation
    (15.0, None),          // 15s - no aggregation
    (30.0, None),          // 30s - no aggregation
    (60.0, None),          // 1m  - no aggregation
    (300.0, Some(5.0)),    // 5m  - 5s buckets
    (900.0, Some(15.0)),   // 15m - 15s buckets
    (1800.0, Some(30.0)),  // 30m - 30s buckets
    (3600.0, Some(60.0)),  // 1h  - 1m buckets
    (7200.0, Some(120.0)), // 2h  - 2m buckets
    (21600.0, Some(300.0)), // 6h  - 5m buckets
    (43200.0, Some(600.0)), // 12h - 10m buckets
    (86400.0, Some(1200.0)), // 1d  - 20m buckets
];

/// Chart zoom/pan state
pub struct ChartState {
    /// Current zoom level index into ZOOM_LEVELS
    zoom_index: usize,
    /// Pan offset from the end (0 = latest data on right edge)
    pub pan_offset_secs: f64,
    /// Total duration of data available
    pub total_duration_secs: f64,
    /// Chart visualization type
    pub chart_type: ChartType,
}

impl Default for ChartState {
    fn default() -> Self {
        ChartState {
            zoom_index: 4, // Default to 1m (60s)
            pan_offset_secs: 0.0,
            total_duration_secs: 0.0,
            chart_type: ChartType::Line,
        }
    }
}

impl ChartState {
    /// Create a ChartState for a given data duration, picking appropriate starting zoom
    pub fn for_duration(duration_secs: f64) -> Self {
        // Find the smallest zoom level that fits the data, or default to 1m
        let zoom_index = ZOOM_LEVELS
            .iter()
            .position(|(w, _)| *w >= duration_secs)
            .unwrap_or(4) // Default to 1m if duration is very short
            .min(4); // Start at 1m max, user can zoom out

        ChartState {
            zoom_index,
            pan_offset_secs: 0.0,
            total_duration_secs: duration_secs,
            chart_type: ChartType::Line,
        }
    }

    /// Toggle between line and bar chart
    pub fn toggle_chart_type(&mut self) {
        self.chart_type = match self.chart_type {
            ChartType::Line => ChartType::Bar,
            ChartType::Bar => ChartType::Line,
        };
    }
}

impl ChartState {
    /// Get current window size in seconds
    pub fn window_secs(&self) -> f64 {
        ZOOM_LEVELS[self.zoom_index].0
    }

    /// Get aggregation bucket size, or None if no aggregation needed
    pub fn aggregation_bucket(&self) -> Option<f64> {
        ZOOM_LEVELS[self.zoom_index].1
    }

    /// Get human-readable zoom level label
    pub fn zoom_label(&self) -> String {
        let secs = self.window_secs();
        if secs >= 86400.0 {
            format!("{}d", (secs / 86400.0) as u32)
        } else if secs >= 3600.0 {
            format!("{}h", (secs / 3600.0) as u32)
        } else if secs >= 60.0 {
            format!("{}m", (secs / 60.0) as u32)
        } else {
            format!("{}s", secs as u32)
        }
    }

    pub fn zoom_in(&mut self) {
        if self.zoom_index > 0 {
            self.zoom_index -= 1;
            // Adjust pan to keep view reasonable
            self.clamp_pan();
        }
    }

    pub fn zoom_out(&mut self) {
        // Allow zooming to any level - useful for live mode where you want
        // to set up a view before data accumulates
        if self.zoom_index < ZOOM_LEVELS.len() - 1 {
            self.zoom_index += 1;
            self.clamp_pan();
        }
    }

    fn clamp_pan(&mut self) {
        let max_pan = (self.total_duration_secs - self.window_secs()).max(0.0);
        self.pan_offset_secs = self.pan_offset_secs.min(max_pan);
    }

    pub fn pan_left(&mut self) {
        // Step by bucket size (or 1s if not aggregating) for precise navigation
        let step = self.aggregation_bucket().unwrap_or(1.0);
        let max_pan = (self.total_duration_secs - self.window_secs()).max(0.0);
        self.pan_offset_secs = (self.pan_offset_secs + step).min(max_pan);
    }

    pub fn pan_right(&mut self) {
        let step = self.aggregation_bucket().unwrap_or(1.0);
        self.pan_offset_secs = (self.pan_offset_secs - step).max(0.0);
    }

    /// Big pan left (1/4 of window)
    pub fn pan_left_big(&mut self) {
        let step = self.window_secs() / 4.0;
        let max_pan = (self.total_duration_secs - self.window_secs()).max(0.0);
        self.pan_offset_secs = (self.pan_offset_secs + step).min(max_pan);
    }

    /// Big pan right (1/4 of window)
    pub fn pan_right_big(&mut self) {
        let step = self.window_secs() / 4.0;
        self.pan_offset_secs = (self.pan_offset_secs - step).max(0.0);
    }

    /// Pan to beginning of data
    pub fn pan_to_start(&mut self) {
        let max_pan = (self.total_duration_secs - self.window_secs()).max(0.0);
        self.pan_offset_secs = max_pan;
    }

    /// Pan to end (most recent data)
    pub fn pan_to_end(&mut self) {
        self.pan_offset_secs = 0.0;
    }

    /// Get the visible time range (start, end) in seconds
    /// Always returns a full window-width range to maintain consistent scaling
    /// Data appears on the right side when there's less data than the window
    pub fn visible_range(&self, elapsed_secs: f64) -> (f64, f64) {
        let end = elapsed_secs - self.pan_offset_secs;
        let start = end - self.window_secs();
        // Don't clamp start to 0 - allow negative to keep window size fixed
        // This puts recent data on the right, empty space on the left
        (start, end)
    }
}

/// TUI Application state - supports both live and static modes
pub struct App {
    // Live mode components (None in static/view mode)
    sampler: Option<CpuSampler>,
    heap_sampler: Option<HeapSampler>,
    resolver: Option<SymbolResolver>,
    storage: Option<Storage>,
    // Static mode: read-only DB connection
    conn: Option<Connection>,

    checkpoint_interval: Duration,
    max_duration: Option<Duration>,
    start_time: Instant,
    last_checkpoint: Instant,
    total_samples: u64,
    running: bool,
    paused: bool,

    // Selection state
    selected_row: usize,
    scroll_offset: usize,
    selected_location_id: Option<i64>,
    selected_func_name: Option<String>,
    func_history: Vec<(f64, f64)>,
    last_history_tick: Instant,
    cached_entries: Vec<CpuEntry>,
    cached_heap_entries: Vec<HeapEntry>,
    table_area: Rect,
    chart_area: Rect,
    chart_data_cache: ChartDataCache,

    // Chart zoom/pan state
    pub chart_state: ChartState,
    // Focus for keyboard navigation
    pub focus: Focus,
    // Static mode: total duration from DB
    static_duration_secs: f64,
    // File name for display (static mode)
    file_name: Option<String>,
    // View mode (CPU, Memory, Both)
    pub view_mode: ViewMode,
    // Sort column for Both mode
    pub sort_column: SortColumn,
}

impl App {
    /// Create a new live profiling app
    pub fn new(
        sampler: CpuSampler,
        heap_sampler: Option<HeapSampler>,
        resolver: SymbolResolver,
        storage: Storage,
        checkpoint_interval: Duration,
        max_duration: Option<Duration>,
    ) -> Self {
        App {
            sampler: Some(sampler),
            heap_sampler,
            resolver: Some(resolver),
            storage: Some(storage),
            conn: None,
            checkpoint_interval,
            max_duration,
            start_time: Instant::now(),
            last_checkpoint: Instant::now(),
            total_samples: 0,
            running: true,
            paused: false,
            selected_row: 0,
            scroll_offset: 0,
            selected_location_id: None,
            selected_func_name: None,
            func_history: Vec::new(),
            last_history_tick: Instant::now(),
            cached_entries: Vec::new(),
            cached_heap_entries: Vec::new(),
            table_area: Rect::default(),
            chart_area: Rect::default(),
            chart_data_cache: ChartDataCache::default(),
            chart_state: ChartState::default(),
            focus: Focus::Table,
            static_duration_secs: 0.0,
            file_name: None,
            view_mode: ViewMode::default(),
            sort_column: SortColumn::default(),
        }
    }

    /// Create a static viewer app from a profile database
    pub fn from_file(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Load metadata
        let total_samples: i64 = conn
            .query_row("SELECT COALESCE(SUM(count), 0) FROM cpu_samples", [], |row| row.get(0))
            .unwrap_or(0);

        let duration_ms: i64 = conn
            .query_row("SELECT COALESCE(MAX(timestamp_ms), 0) FROM checkpoints", [], |row| row.get(0))
            .unwrap_or(0);

        let duration_secs = duration_ms as f64 / 1000.0;

        // Load all entries
        let entries = crate::storage::query_top_cpu(&conn, 1000, 0.0)?;
        let heap_entries = crate::storage::query_top_heap_live(&conn, 100).unwrap_or_default();

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string());

        let mut app = App {
            sampler: None,
            heap_sampler: None,
            resolver: None,
            storage: None,
            conn: Some(conn),
            checkpoint_interval: Duration::from_secs(1),
            max_duration: None,
            start_time: Instant::now(),
            last_checkpoint: Instant::now(),
            total_samples: total_samples as u64,
            running: true,
            paused: true, // Static mode is always "paused"
            selected_row: 0,
            scroll_offset: 0,
            selected_location_id: None,
            selected_func_name: None,
            func_history: Vec::new(),
            last_history_tick: Instant::now(),
            cached_entries: entries,
            cached_heap_entries: heap_entries,
            table_area: Rect::default(),
            chart_area: Rect::default(),
            chart_data_cache: ChartDataCache::default(),
            chart_state: ChartState::for_duration(duration_secs),
            focus: Focus::Table,
            static_duration_secs: duration_secs,
            file_name,
            view_mode: ViewMode::default(),
            sort_column: SortColumn::default(),
        };

        // Load initial timeseries for first entry
        if !app.cached_entries.is_empty() {
            let loc_id = app.cached_entries[0].location_id;
            let func_name = app.cached_entries[0].function.clone();
            app.load_timeseries_static(loc_id, &func_name);
        }

        Ok(app)
    }

    /// Check if this is a static/view mode app
    pub fn is_static(&self) -> bool {
        self.conn.is_some()
    }

    /// Get file name for static mode
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// Enable heap profiling (requires the 'heap' feature and CAP_BPF)
    pub fn enable_heap_profiling(&mut self, exe_path: &std::path::Path, pid: u32) -> Result<()> {
        match HeapSampler::new(pid, exe_path) {
            Ok(sampler) => {
                self.heap_sampler = Some(sampler);
                Ok(())
            }
            Err(e) => {
                // Log warning but don't fail - heap profiling is optional
                eprintln!("Warning: Heap profiling unavailable: {}", e);
                Err(e)
            }
        }
    }

    /// Check if heap profiling is active
    pub fn has_heap_profiling(&self) -> bool {
        self.heap_sampler.is_some()
    }

    pub fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main loop
        let result = self.main_loop(&mut terminal);

        // Restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;

        result
    }

    fn main_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        while self.running {
            // Check duration limit (live mode only)
            if !self.is_static() {
                if let Some(max) = self.max_duration {
                    if self.start_time.elapsed() >= max {
                        break;
                    }
                }
            }

            // Handle input
            let poll_duration = if self.is_static() {
                Duration::from_millis(50) // Static mode: slower polling, less CPU
            } else {
                Duration::from_millis(10)
            };

            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key.code, key.modifiers);
                        }
                    }
                    Event::Mouse(mouse) => {
                        let ctrl = mouse.modifiers.contains(KeyModifiers::CONTROL);
                        match mouse.kind {
                            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                self.handle_click(mouse.column, mouse.row);
                            }
                            MouseEventKind::ScrollUp => {
                                if self.focus == Focus::Table {
                                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                                } else if ctrl {
                                    // Ctrl+scroll = zoom on chart
                                    self.chart_state.zoom_in();
                                } else {
                                    // Scroll = pan on chart
                                    self.chart_state.pan_left();
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if self.focus == Focus::Table {
                                    self.scroll_offset = self.scroll_offset.saturating_add(3);
                                } else if ctrl {
                                    // Ctrl+scroll = zoom on chart
                                    self.chart_state.zoom_out();
                                } else {
                                    // Scroll = pan on chart
                                    self.chart_state.pan_right();
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }

            // Live mode: read samples and update
            if !self.is_static() && !self.paused {
                if let (Some(sampler), Some(resolver), Some(storage)) =
                    (self.sampler.as_mut(), self.resolver.as_ref(), self.storage.as_mut())
                {
                    // Read CPU samples
                    let samples = sampler.read_samples()?;
                    self.total_samples += samples.len() as u64;

                    for addr in samples {
                        let location = resolver.resolve(addr);
                        storage.record_cpu_sample(addr, &location);
                    }

                    if self.last_checkpoint.elapsed() >= self.checkpoint_interval {
                        storage.flush_checkpoint()?;
                        self.last_checkpoint = Instant::now();
                    }
                }

                // Read heap stats if available
                if let (Some(hs), Some(resolver), Some(storage)) =
                    (self.heap_sampler.as_ref(), self.resolver.as_ref(), self.storage.as_mut())
                {
                    let heap_stats = hs.read_stats();
                    let inline_stacks = hs.read_inline_stacks();

                    for (key_addr, stats) in heap_stats {
                        // Use inline stack for better resolution if available
                        let location = if let Some(stack) = inline_stacks.get(&key_addr) {
                            find_user_frame(stack, resolver)
                        } else {
                            resolver.resolve(key_addr)
                        };
                        storage.record_heap_sample(
                            &location,
                            stats.total_alloc_bytes as i64,
                            stats.total_free_bytes as i64,
                            stats.live_bytes,
                        );
                    }
                }
            }

            // Update entries and selection (live mode queries DB each frame)
            if !self.is_static() {
                if let Some(storage) = &self.storage {
                    self.cached_entries = storage.query_top_cpu_live(100);
                    self.cached_heap_entries = storage.query_top_heap_live(100);
                    // Update chart total duration
                    self.chart_state.total_duration_secs = self.start_time.elapsed().as_secs_f64();
                }
            }

            // Update selection state
            if !self.cached_entries.is_empty() {
                // If we have a selected location, find its current row index
                if let Some(loc_id) = self.selected_location_id {
                    if let Some(idx) = self.cached_entries.iter().position(|e| e.location_id == loc_id) {
                        self.selected_row = idx;
                    }
                }

                // Clamp selected row to valid range
                self.selected_row = self.selected_row.min(self.cached_entries.len() - 1);

                // Clamp scroll offset to valid range
                let visible_height = self.table_area.height.saturating_sub(3) as usize;
                let max_scroll = self.cached_entries.len().saturating_sub(visible_height.max(1));
                self.scroll_offset = self.scroll_offset.min(max_scroll);

                // Update timeseries for selected function
                let location_id = self.cached_entries[self.selected_row].location_id;
                let func_name = self.cached_entries[self.selected_row].function.clone();

                if self.is_static() {
                    self.load_timeseries_static(location_id, &func_name);
                } else {
                    let instant_pct = self.cached_entries[self.selected_row].instant_percent;
                    self.update_func_history(location_id, &func_name, instant_pct);
                }
            }

            // Render UI
            terminal.draw(|frame| {
                ui::render(frame, self);
            })?;
        }

        // Final flush (live mode only)
        if let Some(storage) = self.storage.as_mut() {
            storage.flush_checkpoint()?;
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);

        match key {
            // Global controls
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            KeyCode::Char('p') if !self.is_static() => self.paused = !self.paused,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Table => Focus::Chart,
                    Focus::Chart => Focus::Table,
                };
            }

            // === VIEW MODE CONTROLS ===
            // 1/2/3 - direct view selection
            KeyCode::Char('1') => {
                self.view_mode = ViewMode::Cpu;
            }
            KeyCode::Char('2') => {
                self.view_mode = ViewMode::Memory;
            }
            KeyCode::Char('3') => {
                self.view_mode = ViewMode::Both;
            }
            // m - cycle through view modes
            KeyCode::Char('m') => {
                self.view_mode = match self.view_mode {
                    ViewMode::Cpu => ViewMode::Memory,
                    ViewMode::Memory => ViewMode::Both,
                    ViewMode::Both => ViewMode::Cpu,
                };
            }
            // s - toggle sort column (Both mode only)
            KeyCode::Char('s') if self.view_mode == ViewMode::Both => {
                self.sort_column = match self.sort_column {
                    SortColumn::Cpu => SortColumn::Memory,
                    SortColumn::Memory => SortColumn::Cpu,
                };
            }

            // === TABLE CONTROLS (vim-style) ===
            // j/k or arrows - move selection
            KeyCode::Char('j') | KeyCode::Down if self.focus == Focus::Table => {
                self.move_selection(1);
            }
            KeyCode::Char('k') | KeyCode::Up if self.focus == Focus::Table => {
                self.move_selection(-1);
            }
            // Ctrl+d/u - half page
            KeyCode::Char('d') if ctrl && self.focus == Focus::Table => {
                self.move_selection(self.half_page() as i32);
            }
            KeyCode::Char('u') if ctrl && self.focus == Focus::Table => {
                self.move_selection(-(self.half_page() as i32));
            }
            // Ctrl+f/b - full page
            KeyCode::Char('f') if ctrl && self.focus == Focus::Table => {
                self.move_selection(self.full_page() as i32);
            }
            KeyCode::Char('b') if ctrl && self.focus == Focus::Table => {
                self.move_selection(-(self.full_page() as i32));
            }
            // gg/G - top/bottom
            KeyCode::Char('g') if self.focus == Focus::Table => {
                self.selected_row = 0;
                self.selected_location_id = None;
                self.ensure_selection_visible();
            }
            KeyCode::Char('G') if self.focus == Focus::Table => {
                self.selected_row = self.cached_entries.len().saturating_sub(1);
                self.selected_location_id = None;
                self.ensure_selection_visible();
            }
            KeyCode::Home if self.focus == Focus::Table => {
                self.selected_row = 0;
                self.selected_location_id = None;
                self.ensure_selection_visible();
            }
            KeyCode::End if self.focus == Focus::Table => {
                self.selected_row = self.cached_entries.len().saturating_sub(1);
                self.selected_location_id = None;
                self.ensure_selection_visible();
            }

            // === CHART CONTROLS (vim-style) ===
            // h/l or arrows - pan
            KeyCode::Char('h') | KeyCode::Left if self.focus == Focus::Chart && !ctrl => {
                self.chart_state.pan_left();
            }
            KeyCode::Char('l') | KeyCode::Right if self.focus == Focus::Chart && !ctrl => {
                self.chart_state.pan_right();
            }
            // Ctrl+h/l or Ctrl+arrows - big pan (1/4 screen)
            KeyCode::Char('h') | KeyCode::Left if self.focus == Focus::Chart && ctrl => {
                self.chart_state.pan_left_big();
            }
            KeyCode::Char('l') | KeyCode::Right if self.focus == Focus::Chart && ctrl => {
                self.chart_state.pan_right_big();
            }
            // +/= and - for zoom
            KeyCode::Char('+') | KeyCode::Char('=') if self.focus == Focus::Chart => {
                self.chart_state.zoom_in();
            }
            KeyCode::Char('-') if self.focus == Focus::Chart => {
                self.chart_state.zoom_out();
            }
            // k/j or up/down for zoom (vim style - up zooms in)
            KeyCode::Char('k') | KeyCode::Up if self.focus == Focus::Chart => {
                self.chart_state.zoom_in();
            }
            KeyCode::Char('j') | KeyCode::Down if self.focus == Focus::Chart => {
                self.chart_state.zoom_out();
            }
            // 0/^ and $ - start/end
            KeyCode::Char('0') | KeyCode::Char('^') | KeyCode::Home if self.focus == Focus::Chart => {
                self.chart_state.pan_to_start();
            }
            KeyCode::Char('$') | KeyCode::End if self.focus == Focus::Chart => {
                self.chart_state.pan_to_end();
            }
            // Spacebar - jump to end (resume following live data)
            KeyCode::Char(' ') if self.focus == Focus::Chart => {
                self.chart_state.pan_to_end();
            }
            // b - toggle between line and bar chart
            KeyCode::Char('b') if self.focus == Focus::Chart => {
                self.chart_state.toggle_chart_type();
            }

            _ => {}
        }
    }

    /// Move table selection by delta rows (positive = down, negative = up)
    fn move_selection(&mut self, delta: i32) {
        let new_row = if delta >= 0 {
            self.selected_row.saturating_add(delta as usize)
        } else {
            self.selected_row.saturating_sub((-delta) as usize)
        };
        self.selected_row = new_row.min(self.cached_entries.len().saturating_sub(1));
        self.selected_location_id = None;
        self.ensure_selection_visible();
    }

    /// Get half page size for Ctrl+d/u
    fn half_page(&self) -> usize {
        let visible = self.table_area.height.saturating_sub(3) as usize;
        (visible / 2).max(1)
    }

    /// Get full page size for Ctrl+f/b
    fn full_page(&self) -> usize {
        let visible = self.table_area.height.saturating_sub(3) as usize;
        visible.max(1)
    }

    /// Ensure the selected row is visible by adjusting scroll offset
    fn ensure_selection_visible(&mut self) {
        let visible_height = self.table_area.height.saturating_sub(3) as usize;
        if visible_height == 0 {
            return;
        }

        // If selection is above visible area, scroll up
        if self.selected_row < self.scroll_offset {
            self.scroll_offset = self.selected_row;
        }
        // If selection is below visible area, scroll down
        else if self.selected_row >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected_row - visible_height + 1;
        }
    }

    fn handle_click(&mut self, x: u16, y: u16) {
        // Check if click is within table area
        if x >= self.table_area.x
            && x < self.table_area.x + self.table_area.width
            && y >= self.table_area.y
            && y < self.table_area.y + self.table_area.height
        {
            self.focus = Focus::Table;

            // Calculate row index from click position
            // Table has: border (1) + header (1) = 2 rows before data
            let table_header_offset = 2u16;
            let click_row = y.saturating_sub(self.table_area.y).saturating_sub(table_header_offset);

            // Convert visual row to actual entry index using current scroll offset
            let clicked_index = self.scroll_offset + click_row as usize;

            if clicked_index < self.cached_entries.len() {
                self.selected_row = clicked_index;
                self.selected_location_id = None; // Clear so we pick up new location
            }
        }
        // Check if click is within chart area
        else if x >= self.chart_area.x
            && x < self.chart_area.x + self.chart_area.width
            && y >= self.chart_area.y
            && y < self.chart_area.y + self.chart_area.height
        {
            self.focus = Focus::Chart;
        }
    }

    // Getters for UI
    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn selected_row(&self) -> usize {
        self.selected_row
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn entries(&self) -> &[crate::storage::CpuEntry] {
        &self.cached_entries
    }

    pub fn heap_entries(&self) -> &[crate::storage::HeapEntry] {
        &self.cached_heap_entries
    }

    pub fn func_history(&self) -> &[(f64, f64)] {
        &self.func_history
    }

    pub fn selected_func(&self) -> Option<&str> {
        self.selected_func_name.as_deref()
    }

    /// Set the table area for mouse click detection
    pub fn set_table_area(&mut self, area: Rect) {
        self.table_area = area;
    }

    /// Set the chart area for mouse click detection
    pub fn set_chart_area(&mut self, area: Rect) {
        self.chart_area = area;
    }

    /// Update function history from DB (live mode)
    pub fn update_func_history(&mut self, location_id: i64, func_name: &str, _cpu_pct: f64) {
        let location_changed = self.selected_location_id != Some(location_id);
        if location_changed {
            self.selected_location_id = Some(location_id);
            self.selected_func_name = Some(func_name.to_string());
            self.chart_data_cache.location_id = None; // Invalidate cache
        }

        // Re-query DB every checkpoint interval to get fresh data
        // Also invalidate chart cache so it gets fresh data
        if location_changed || self.last_history_tick.elapsed() >= self.checkpoint_interval {
            self.chart_data_cache.location_id = None; // Invalidate for fresh data
            if let Some(storage) = &self.storage {
                self.func_history = storage.query_location_timeseries(location_id);
                self.last_history_tick = Instant::now();
            }
        }
    }

    /// Load timeseries for static/view mode
    fn load_timeseries_static(&mut self, location_id: i64, func_name: &str) {
        let location_changed = self.selected_location_id != Some(location_id);
        if !location_changed {
            return; // Already loaded
        }

        self.selected_location_id = Some(location_id);
        self.selected_func_name = Some(func_name.to_string());
        self.chart_data_cache.location_id = None; // Invalidate cache

        if let Some(conn) = &self.conn {
            self.func_history = crate::storage::query_cpu_timeseries(conn, location_id)
                .map(|points| {
                    points
                        .into_iter()
                        .map(|p| (p.timestamp_ms as f64 / 1000.0, p.percent))
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    /// Query chart data with DB-level aggregation and caching
    /// Returns data aggregated to match the chart's screen columns
    /// Prefetches 3x the visible window for smooth scrolling
    pub fn query_chart_data(&mut self, visible_start: f64, visible_end: f64, num_columns: usize) -> &[(f64, f64)] {
        let location_id = match self.selected_location_id {
            Some(id) => id,
            None => return &[],
        };

        let visible_range = visible_end - visible_start;
        let points_per_sec = num_columns as f64 / visible_range;

        // Check if cache is valid:
        // - Same location
        // - Visible range is within cached range
        // - Resolution is similar (within 20%)
        let cache_valid = self.chart_data_cache.location_id == Some(location_id)
            && visible_start >= self.chart_data_cache.cache_start_secs
            && visible_end <= self.chart_data_cache.cache_end_secs
            && (self.chart_data_cache.points_per_sec - points_per_sec).abs() / points_per_sec.max(0.001) < 0.2;

        if !cache_valid {
            // Prefetch 3x the visible window (1 before, visible, 1 after)
            let prefetch_start = (visible_start - visible_range).max(0.0);
            let prefetch_end = visible_end + visible_range;
            let prefetch_range = prefetch_end - prefetch_start;

            // Calculate number of buckets for prefetch window
            let num_buckets = ((prefetch_range / visible_range) * num_columns as f64).ceil() as usize;

            let start_ms = (prefetch_start * 1000.0) as i64;
            let end_ms = (prefetch_end * 1000.0) as i64;

            // Query from DB with aggregation
            let data = if let Some(storage) = &self.storage {
                storage.query_location_timeseries_aggregated(location_id, start_ms, end_ms, num_buckets)
            } else if let Some(conn) = &self.conn {
                query_cpu_timeseries_aggregated(conn, location_id, start_ms, end_ms, num_buckets)
            } else {
                Vec::new()
            };

            // Update cache
            self.chart_data_cache.location_id = Some(location_id);
            self.chart_data_cache.cache_start_secs = prefetch_start;
            self.chart_data_cache.cache_end_secs = prefetch_end;
            self.chart_data_cache.points_per_sec = points_per_sec;
            self.chart_data_cache.data = data;
        }

        &self.chart_data_cache.data
    }

    /// Invalidate the chart data cache (call when location changes or data is updated)
    pub fn invalidate_chart_cache(&mut self) {
        self.chart_data_cache.location_id = None;
    }

    /// Get elapsed time (or total duration in static mode)
    pub fn elapsed(&self) -> Duration {
        if self.is_static() {
            Duration::from_secs_f64(self.static_duration_secs)
        } else {
            self.start_time.elapsed()
        }
    }

    /// Get elapsed seconds as f64
    pub fn elapsed_secs(&self) -> f64 {
        if self.is_static() {
            self.static_duration_secs
        } else {
            self.start_time.elapsed().as_secs_f64()
        }
    }

    /// Get number of entries for scroll bounds
    pub fn entry_count(&self) -> usize {
        self.cached_entries.len()
    }
}
