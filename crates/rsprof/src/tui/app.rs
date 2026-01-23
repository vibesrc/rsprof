use crate::cpu::CpuSampler;
use crate::error::Result;
use crate::heap::ShmHeapSampler;
use crate::storage::{CpuEntry, HeapEntry, Storage, query_cpu_timeseries_aggregated};
use crate::symbols::SymbolResolver;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, prelude::*};
use rusqlite::Connection;
use std::collections::{HashMap, VecDeque};
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
    checkpoint_seq: u64,
}

/// Cache for heap chart data
#[derive(Default)]
struct HeapChartCache {
    location_id: Option<i64>,
    cache_start_secs: f64,
    cache_end_secs: f64,
    points_per_sec: f64,
    data: Vec<(f64, f64)>,
    checkpoint_seq: u64,
}

struct LocationInfo {
    file: String,
    line: u32,
    function: String,
}

use super::ui;

/// Patterns for internal/profiler functions to skip
const SKIP_FUNCTION_PATTERNS: &[&str] = &[
    // Rust allocator entry points
    "__rust_alloc",
    "__rust_dealloc",
    "__rust_realloc",
    "__rustc",
    // Rust alloc crate internals
    "alloc::alloc::",
    "alloc::raw_vec::",
    "alloc::vec::",
    "alloc::string::",
    "alloc::collections::",
    "<alloc::",
    "alloc::fmt::",
    "alloc::ffi::", // format! and CString internals
    // Hashmap/collections internals
    "hashbrown::",
    "std::collections::hash",
    // Core library internals
    "core::ptr::",
    "core::slice::",
    "core::iter::",
    "<core::",
    "core::ops::function::",
    "core::ops::drop::",
    "core::ffi::",
    "core::fmt::",
    "core::num::",
    "core::str::",
    "core::hash::",
    "core::mem::",
    // Std library internals
    "std::io::",
    "std::fmt::",
    "std::sys::",
    "std::thread::",
    "std::sync::",
    "<std::",
    "fmt::num::",
    "fmt::Write::",
    // Trait implementations (raw DWARF names)
    " as core::fmt::",  // <T as core::fmt::Display>::fmt
    " as std::fmt::",   // <T as std::fmt::Write>::write
    " as core::hash::", // <T as core::hash::Hash>::hash
    " as alloc::",      // <T as alloc::*>::method
    // Trait implementations on generic types
    "<_>::", // any method on trait objects
    // Libc functions
    "malloc",
    "calloc",
    "realloc",
    "free",
    "memcpy",
    "memmove",
    "memset",
    "memchr",
    "_start",
    "__libc_start_main",
    // Exception/unwinding
    "_Unwind_",
    "__cxa_",
    "_fini",
    "_init",
    "rust_eh_personality",
    // Profiler internals (rsprof-trace)
    "addr2line::",
    "gimli::",
    "object::",
    "miniz_oxide::",
    "rustc_demangle::",
    "rsprof_alloc::",
    "rsprof_trace::",
    "profiling::",
    "rsprof::",
    // Sorting internals
    "sort::shared::smallsort::",
    // Generic patterns for generated code
    "::{{closure}}", // closures attributed to parent
];

const SPARKLINE_WIDTH: u64 = 12;

/// Check if a file path looks like internal/library code
fn is_internal_file(file: &str) -> bool {
    file.is_empty()
        || file.starts_with('[')
        || file.starts_with('<')
        || file.contains("/rustc/")
        || file.contains("/.cargo/registry/")
        || file.contains("/rust/library/")
        || file.contains("rsprof-alloc")
        || file.contains("rsprof-trace")
        || file.contains("profiling.rs")
        || file == "lib.rs"
        || file == "time.rs"
        || file == "unix.rs"
        || file.ends_with("memchr.rs")
        || file.ends_with("maybe_uninit.rs")
        || file.ends_with("methods.rs")
        || (file.ends_with("mod.rs") && !file.contains("/src/"))
}

/// Check if a location is internal (profiler/library code)
fn is_internal_location(loc: &crate::symbols::Location) -> bool {
    if is_internal_file(&loc.file) {
        return true;
    }
    SKIP_FUNCTION_PATTERNS
        .iter()
        .any(|p| loc.function.contains(p))
}

/// Patterns for utility functions that should be attributed to their callers
const UTILITY_PATTERNS: &[&str] = &[
    // Derived trait methods - attribute to caller
    ">::clone",       // Clone::clone on any type
    ">::fmt",         // Debug/Display::fmt
    ">::hash",        // Hash::hash
    ">::eq",          // PartialEq::eq
    ">::partial_cmp", // PartialOrd
    ">::cmp",         // Ord
    // Common utility functions
    "::utils::",
    "::to_string",
    "::to_owned",
    "::into",
];

/// Check if a function is a utility function (should attribute to caller)
fn is_utility_function(func: &str) -> bool {
    UTILITY_PATTERNS.iter().any(|p| func.contains(p))
}

/// Find the first "user" frame in a stack trace (not allocator internals)
/// If the first user frame is a utility function, return its caller instead.
fn find_user_frame(stack: &[u64], resolver: &SymbolResolver) -> crate::symbols::Location {
    let mut first_user_frame: Option<crate::symbols::Location> = None;
    let mut first_user_idx: Option<usize> = None;

    // FIRST PASS: Find the first user frame
    for (i, &addr) in stack.iter().enumerate() {
        let loc = resolver.resolve(addr);
        // Skip internal functions based on name patterns
        let has_internal_fn = SKIP_FUNCTION_PATTERNS
            .iter()
            .any(|p| loc.function.contains(p));
        if !has_internal_fn
            && !is_internal_file(&loc.file)
            && !loc.function.is_empty()
            && loc.function != "[unknown]"
        {
            first_user_frame = Some(loc);
            first_user_idx = Some(i);
            break;
        }
    }

    // SECOND PASS: If first user frame is a utility function, find its caller
    if let (Some(first), Some(idx)) = (&first_user_frame, first_user_idx) {
        if is_utility_function(&first.function) {
            // Look for the caller (next frame that's not internal)
            for &addr in stack.iter().skip(idx + 1) {
                let loc = resolver.resolve(addr);
                let has_internal_fn = SKIP_FUNCTION_PATTERNS
                    .iter()
                    .any(|p| loc.function.contains(p));
                if !has_internal_fn && !loc.function.is_empty() && loc.function != "[unknown]" {
                    return loc;
                }
            }
        }
        return first_user_frame.unwrap();
    }

    // Fallback: look for frames with real source paths
    for &addr in stack {
        let loc = resolver.resolve(addr);
        if !is_internal_file(&loc.file) && !is_internal_location(&loc) {
            return loc;
        }
    }

    // Last resort: first address
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortColumn {
    Total,
    Live,
    Function,
    Location,
    Trend,
}

#[derive(Clone, Copy, Debug)]
pub struct TableSort {
    pub column: SortColumn,
    pub descending: bool,
}

impl TableSort {
    fn default_cpu() -> Self {
        TableSort {
            column: SortColumn::Total,
            descending: true,
        }
    }

    fn default_heap() -> Self {
        TableSort {
            column: SortColumn::Live,
            descending: true,
        }
    }
}

/// View mode for switching between CPU and Memory views
#[derive(Clone, Copy, PartialEq, Default)]
pub enum ViewMode {
    #[default]
    Cpu,
    Memory,
}

/// Fixed zoom levels with corresponding aggregation bucket sizes
/// (window_secs, bucket_secs) - bucket is None if no aggregation needed
const ZOOM_LEVELS: &[(f64, Option<f64>)] = &[
    (5.0, Some(1.0)),        // 5s  - 1s buckets
    (10.0, Some(1.0)),       // 10s - 1s buckets
    (15.0, Some(1.0)),       // 15s - 1s buckets
    (30.0, Some(1.0)),       // 30s - 1s buckets
    (60.0, Some(1.0)),       // 1m  - 1s buckets
    (300.0, Some(5.0)),      // 5m  - 5s buckets
    (900.0, Some(15.0)),     // 15m - 15s buckets
    (1800.0, Some(30.0)),    // 30m - 30s buckets
    (3600.0, Some(60.0)),    // 1h  - 1m buckets
    (7200.0, Some(120.0)),   // 2h  - 2m buckets
    (21600.0, Some(300.0)),  // 6h  - 5m buckets
    (43200.0, Some(600.0)),  // 12h - 10m buckets
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
    /// Whether Y-axis starts from zero (false = auto-scale)
    pub y_axis_from_zero: bool,
}

impl Default for ChartState {
    fn default() -> Self {
        ChartState {
            zoom_index: 4, // Default to 1m (60s)
            pan_offset_secs: 0.0,
            total_duration_secs: 0.0,
            chart_type: ChartType::Line,
            y_axis_from_zero: false, // Auto-scale by default
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
            y_axis_from_zero: false,
        }
    }

    /// Toggle between line and bar chart
    pub fn toggle_chart_type(&mut self) {
        self.chart_type = match self.chart_type {
            ChartType::Line => ChartType::Bar,
            ChartType::Bar => ChartType::Line,
        };
    }

    /// Toggle Y-axis between auto-scale and starting from zero
    pub fn toggle_y_axis_zero(&mut self) {
        self.y_axis_from_zero = !self.y_axis_from_zero;
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
    shm_heap_sampler: Option<ShmHeapSampler>,
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
    paused_elapsed: Option<Duration>,
    last_draw: Instant,
    last_click: Option<(Instant, u16, u16)>,
    include_internal: bool,

    // Selection state
    selected_row: usize,
    scroll_offset: usize,
    selected_location_id: Option<i64>,
    selected_heap_location_id: Option<i64>,
    selected_func_name: Option<String>,
    cpu_sort: TableSort,
    heap_sort: TableSort,
    func_history: Vec<(f64, f64)>,
    last_history_tick: Instant,
    live_cpu_totals: HashMap<i64, u64>,
    live_cpu_instant: HashMap<i64, u64>,
    location_info: HashMap<i64, LocationInfo>,
    cpu_last_seen: HashMap<i64, u64>,
    heap_live_entries: HashMap<i64, HeapEntry>,
    heap_last_seen: HashMap<i64, u64>,
    chart_checkpoint_seq: u64,
    cached_entries: Vec<CpuEntry>,
    cached_heap_entries: Vec<HeapEntry>,
    cached_cpu_sparklines: HashMap<i64, VecDeque<i64>>,
    cached_heap_sparklines: HashMap<i64, VecDeque<i64>>,
    table_area: Rect,
    chart_area: Rect,
    chart_data_cache: ChartDataCache,
    heap_chart_cache: HeapChartCache,

    // Chart zoom/pan state
    pub chart_state: ChartState,
    // Focus for keyboard navigation
    pub focus: Focus,
    // Static mode: total duration from DB
    static_duration_secs: f64,
    // File name for display (static mode)
    file_name: Option<String>,
    // View mode (CPU or Memory)
    pub view_mode: ViewMode,
    // Chart visibility (false = full-width table with sparklines)
    pub chart_visible: bool,
}

impl App {
    /// Create a new live profiling app
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        perf_sampler: Option<CpuSampler>,
        shm_sampler: Option<ShmHeapSampler>,
        resolver: SymbolResolver,
        storage: Storage,
        checkpoint_interval: Duration,
        max_duration: Option<Duration>,
        include_internal: bool,
    ) -> Self {
        App {
            sampler: perf_sampler,
            shm_heap_sampler: shm_sampler,
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
            paused_elapsed: None,
            last_draw: Instant::now(),
            last_click: None,
            include_internal,
            selected_row: 0,
            scroll_offset: 0,
            selected_location_id: None,
            selected_heap_location_id: None,
            selected_func_name: None,
            cpu_sort: TableSort::default_cpu(),
            heap_sort: TableSort::default_heap(),
            func_history: Vec::new(),
            last_history_tick: Instant::now(),
            live_cpu_totals: HashMap::new(),
            live_cpu_instant: HashMap::new(),
            location_info: HashMap::new(),
            cpu_last_seen: HashMap::new(),
            heap_live_entries: HashMap::new(),
            heap_last_seen: HashMap::new(),
            chart_checkpoint_seq: 0,
            cached_entries: Vec::new(),
            cached_heap_entries: Vec::new(),
            cached_cpu_sparklines: HashMap::new(),
            cached_heap_sparklines: HashMap::new(),
            table_area: Rect::default(),
            chart_area: Rect::default(),
            chart_data_cache: ChartDataCache::default(),
            heap_chart_cache: HeapChartCache::default(),
            chart_state: ChartState::default(),
            focus: Focus::Table,
            static_duration_secs: 0.0,
            file_name: None,
            view_mode: ViewMode::default(),
            chart_visible: false, // Hidden by default, sparklines show in table
        }
    }

    /// Create a static viewer app from a profile database
    pub fn from_file(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Load metadata
        let total_samples: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(count), 0) FROM cpu_samples",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let duration_ms: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(timestamp_ms), 0) FROM checkpoints",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let duration_secs = duration_ms as f64 / 1000.0;

        // Load all entries
        let entries = crate::storage::query_top_cpu(&conn, 1000, 0.0)?;
        let heap_entries = crate::storage::query_top_heap_live(&conn, 100).unwrap_or_default();
        // For static mode, initialize sparklines from DB and convert to VecDeque
        let heap_location_ids: Vec<i64> = heap_entries.iter().map(|e| e.location_id).collect();
        let heap_sparklines_vec =
            crate::storage::query_heap_sparklines_for_locations(&conn, 12, &heap_location_ids);
        let heap_sparklines: HashMap<i64, VecDeque<i64>> = heap_sparklines_vec
            .into_iter()
            .map(|(k, v)| (k, VecDeque::from(v)))
            .collect();

        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string());

        let mut app = App {
            sampler: None,
            shm_heap_sampler: None,
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
            paused_elapsed: None,
            last_draw: Instant::now(),
            last_click: None,
            include_internal: false,
            selected_row: 0,
            scroll_offset: 0,
            selected_location_id: None,
            selected_heap_location_id: None,
            selected_func_name: None,
            cpu_sort: TableSort::default_cpu(),
            heap_sort: TableSort::default_heap(),
            func_history: Vec::new(),
            last_history_tick: Instant::now(),
            live_cpu_totals: HashMap::new(),
            live_cpu_instant: HashMap::new(),
            location_info: HashMap::new(),
            cpu_last_seen: HashMap::new(),
            heap_live_entries: HashMap::new(),
            heap_last_seen: HashMap::new(),
            chart_checkpoint_seq: 0,
            cached_entries: entries,
            cached_heap_entries: heap_entries,
            cached_cpu_sparklines: HashMap::new(),
            cached_heap_sparklines: heap_sparklines,
            table_area: Rect::default(),
            chart_area: Rect::default(),
            chart_data_cache: ChartDataCache::default(),
            heap_chart_cache: HeapChartCache::default(),
            chart_state: ChartState::for_duration(duration_secs),
            focus: Focus::Table,
            static_duration_secs: duration_secs,
            file_name,
            view_mode: ViewMode::default(),
            chart_visible: false, // Hidden by default
        };

        app.sort_all_entries();

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

    /// Check if heap profiling is active
    pub fn has_heap_profiling(&self) -> bool {
        self.shm_heap_sampler.is_some()
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
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        result
    }

    fn main_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        while self.running {
            // Check duration limit (live mode only)
            if !self.is_static()
                && let Some(max) = self.max_duration
                && self.start_time.elapsed() >= max
            {
                break;
            }

            // Handle input
            let poll_duration = if self.is_static() || self.paused {
                Duration::from_millis(80)
            } else {
                Duration::from_millis(20)
            };

            let mut needs_redraw = false;
            let mut checkpointed = false;

            if event::poll(poll_duration)? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key.code, key.modifiers);
                            needs_redraw = true;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let ctrl = mouse.modifiers.contains(KeyModifiers::CONTROL);
                        match mouse.kind {
                            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                if self.is_double_click(mouse.column, mouse.row) {
                                    self.chart_visible = !self.chart_visible;
                                } else {
                                    self.handle_click(mouse.column, mouse.row);
                                }
                                needs_redraw = true;
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
                                needs_redraw = true;
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
                                needs_redraw = true;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }

            // Live mode: read samples and update
            if !self.is_static() && !self.paused {
                let mut did_checkpoint = false;
                let mut heap_entries_map: HashMap<i64, HeapEntry> = HashMap::new();

                // Prefer rsprof-trace SHM sampler (provides both CPU and heap)
                if let Some(shm) = self.shm_heap_sampler.as_mut() {
                    if let (Some(resolver), Some(storage)) =
                        (self.resolver.as_ref(), self.storage.as_mut())
                    {
                        let _events = shm.poll_events(std::time::Duration::from_millis(1));

                        // Process CPU samples from rsprof-trace (aggregated stats)
                        let cpu_stats = shm.read_cpu_stats();
                        let live_cpu_totals = &mut self.live_cpu_totals;
                        let live_cpu_instant = &mut self.live_cpu_instant;
                        let location_info = &mut self.location_info;
                        for (_hash, (count, stack)) in cpu_stats {
                            self.total_samples += count;
                            let location = if self.include_internal {
                                resolve_internal_stack(&stack, resolver)
                            } else {
                                // Walk the stack to find the first user frame (skip allocator/profiler internals)
                                find_user_frame(&stack, resolver)
                            };
                            if self.include_internal || !is_internal_location(&location) {
                                let location_id = storage.record_cpu_sample_count(
                                    stack.first().copied().unwrap_or(0),
                                    &location,
                                    count,
                                );
                                *live_cpu_totals.entry(location_id).or_insert(0) += count;
                                *live_cpu_instant.entry(location_id).or_insert(0) += count;
                                location_info
                                    .entry(location_id)
                                    .or_insert_with(|| LocationInfo {
                                        file: location.file,
                                        line: location.line,
                                        function: location.function,
                                    });
                            }
                        }

                        // Checkpoint - record heap stats and flush
                        if self.last_checkpoint.elapsed() >= self.checkpoint_interval {
                            // Record heap stats from rsprof-trace (once per checkpoint)
                            let heap_stats = shm.read_stats();
                            let inline_stacks = shm.read_inline_stacks();
                            for (key_addr, stats) in heap_stats {
                                let location = if let Some(stack) = inline_stacks.get(&key_addr) {
                                    if self.include_internal {
                                        resolve_internal_stack(stack, resolver)
                                    } else {
                                        find_user_frame(stack, resolver)
                                    }
                                } else if self.include_internal {
                                    crate::symbols::Location::unknown()
                                } else {
                                    resolver.resolve(key_addr)
                                };
                                if self.include_internal || !is_internal_location(&location) {
                                    let location_id = storage.record_heap_sample(
                                        &location,
                                        stats.total_alloc_bytes as i64,
                                        stats.total_free_bytes as i64,
                                        stats.live_bytes,
                                        stats.total_allocs,
                                        stats.total_frees,
                                    );
                                    let entry =
                                        heap_entries_map.entry(location_id).or_insert_with(|| {
                                            HeapEntry {
                                                location_id,
                                                file: location.file,
                                                line: location.line,
                                                function: location.function,
                                                live_bytes: 0,
                                                total_alloc_bytes: 0,
                                                total_free_bytes: 0,
                                                alloc_count: 0,
                                                free_count: 0,
                                            }
                                        });
                                    entry.live_bytes += stats.live_bytes;
                                    entry.total_alloc_bytes += stats.total_alloc_bytes as i64;
                                    entry.total_free_bytes += stats.total_free_bytes as i64;
                                    entry.alloc_count += stats.total_allocs;
                                    entry.free_count += stats.total_frees;
                                }
                            }

                            storage.flush_checkpoint()?;
                            did_checkpoint = true;
                        }
                    }
                }
                // Fallback: Use perf-based CPU sampling
                else if let (Some(sampler), Some(resolver), Some(storage)) = (
                    self.sampler.as_mut(),
                    self.resolver.as_ref(),
                    self.storage.as_mut(),
                ) {
                    let samples = sampler.read_samples()?;
                    self.total_samples += samples.len() as u64;

                    let live_cpu_totals = &mut self.live_cpu_totals;
                    let live_cpu_instant = &mut self.live_cpu_instant;
                    let location_info = &mut self.location_info;
                    for addr in samples {
                        let location = resolver.resolve(addr);
                        if self.include_internal || !is_internal_location(&location) {
                            let location_id = storage.record_cpu_sample(addr, &location);
                            *live_cpu_totals.entry(location_id).or_insert(0) += 1;
                            *live_cpu_instant.entry(location_id).or_insert(0) += 1;
                            location_info
                                .entry(location_id)
                                .or_insert_with(|| LocationInfo {
                                    file: location.file,
                                    line: location.line,
                                    function: location.function,
                                });
                        }
                    }

                    if self.last_checkpoint.elapsed() >= self.checkpoint_interval {
                        storage.flush_checkpoint()?;
                        did_checkpoint = true;
                    }
                }

                if did_checkpoint {
                    self.chart_checkpoint_seq = self.chart_checkpoint_seq.wrapping_add(1);
                    for (location_id, entry) in heap_entries_map {
                        self.heap_live_entries.insert(location_id, entry);
                    }
                    self.last_checkpoint = Instant::now();
                    self.refresh_cpu_entries();
                    let heap_entries: Vec<HeapEntry> =
                        self.heap_live_entries.values().cloned().collect();
                    self.update_heap_entries(heap_entries);
                    self.update_sparklines();
                    // New data available; refresh chart data next time it's rendered.
                    self.chart_data_cache.location_id = None;
                    self.heap_chart_cache.location_id = None;
                    self.live_cpu_instant.clear();
                    checkpointed = true;
                }
            }

            // Update chart duration each frame for smooth zoom bounds
            if !self.is_static() {
                self.chart_state.total_duration_secs = self.start_time.elapsed().as_secs_f64();
            }

            // Update selection state
            match self.view_mode {
                ViewMode::Cpu => {
                    if !self.cached_entries.is_empty() {
                        // If we have a selected location, find its current row index
                        if let Some(loc_id) = self.selected_location_id
                            && let Some(idx) = self
                                .cached_entries
                                .iter()
                                .position(|e| e.location_id == loc_id)
                        {
                            self.selected_row = idx;
                        }

                        // Clamp selected row to valid range
                        self.selected_row = self.selected_row.min(self.cached_entries.len() - 1);

                        // Clamp scroll offset to valid range
                        let visible_height = self.table_area.height.saturating_sub(3) as usize;
                        let max_scroll = self
                            .cached_entries
                            .len()
                            .saturating_sub(visible_height.max(1));
                        self.scroll_offset = self.scroll_offset.min(max_scroll);

                        // Update timeseries for selected function
                        let location_id = self.cached_entries[self.selected_row].location_id;
                        let func_name = self.cached_entries[self.selected_row].function.clone();

                        self.update_selected_cpu(location_id, &func_name);
                        if self.is_static() && self.chart_visible {
                            self.load_timeseries_static(location_id, &func_name);
                        }
                    }
                }
                ViewMode::Memory => {
                    if !self.cached_heap_entries.is_empty() {
                        if let Some(loc_id) = self.selected_heap_location_id {
                            if let Some(idx) = self
                                .cached_heap_entries
                                .iter()
                                .position(|e| e.location_id == loc_id)
                            {
                                self.selected_row = idx;
                            } else {
                                self.selected_heap_location_id = None;
                            }
                        }

                        self.selected_row =
                            self.selected_row.min(self.cached_heap_entries.len() - 1);

                        let visible_height = self.table_area.height.saturating_sub(3) as usize;
                        let max_scroll = self
                            .cached_heap_entries
                            .len()
                            .saturating_sub(visible_height.max(1));
                        self.scroll_offset = self.scroll_offset.min(max_scroll);

                        if self.selected_heap_location_id.is_none() {
                            let location_id =
                                self.cached_heap_entries[self.selected_row].location_id;
                            self.update_selected_heap(location_id);
                        }
                    }
                }
            }

            // Render UI
            let frame_interval = if self.is_static() || self.paused {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(33)
            };
            if needs_redraw || checkpointed || self.last_draw.elapsed() >= frame_interval {
                terminal.draw(|frame| {
                    ui::render(frame, self);
                })?;
                self.last_draw = Instant::now();
            }
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
            KeyCode::Char('c') if ctrl => self.running = false,
            KeyCode::Char('q') => self.running = false,
            KeyCode::Esc => {
                // ESC hides the chart if visible, otherwise does nothing
                if self.chart_visible {
                    self.chart_visible = false;
                }
            }
            KeyCode::Char('p') if !self.is_static() => {
                self.paused = !self.paused;
                if self.paused {
                    self.paused_elapsed = Some(self.start_time.elapsed());
                } else {
                    self.paused_elapsed = None;
                }
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Table => Focus::Chart,
                    Focus::Chart => Focus::Table,
                };
            }

            // === VIEW MODE CONTROLS ===
            // 1/2 - direct view selection
            KeyCode::Char('1') => {
                self.view_mode = ViewMode::Cpu;
            }
            KeyCode::Char('2') => {
                self.view_mode = ViewMode::Memory;
            }
            // m - toggle view mode
            KeyCode::Char('m') => {
                self.view_mode = match self.view_mode {
                    ViewMode::Cpu => ViewMode::Memory,
                    ViewMode::Memory => ViewMode::Cpu,
                };
            }
            // c or Enter - toggle chart visibility
            KeyCode::Char('c') | KeyCode::Enter => {
                self.chart_visible = !self.chart_visible;
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
                self.update_selection_from_row();
                self.ensure_selection_visible();
            }
            KeyCode::Char('G') if self.focus == Focus::Table => {
                self.selected_row = self.active_entry_count().saturating_sub(1);
                self.update_selection_from_row();
                self.ensure_selection_visible();
            }
            KeyCode::Home if self.focus == Focus::Table => {
                self.selected_row = 0;
                self.update_selection_from_row();
                self.ensure_selection_visible();
            }
            KeyCode::End if self.focus == Focus::Table => {
                self.selected_row = self.active_entry_count().saturating_sub(1);
                self.update_selection_from_row();
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
            KeyCode::Char('0') | KeyCode::Char('^') | KeyCode::Home
                if self.focus == Focus::Chart =>
            {
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
            // z - toggle Y-axis between auto-scale and starting from zero
            KeyCode::Char('z') if self.focus == Focus::Chart => {
                self.chart_state.toggle_y_axis_zero();
            }

            _ => {}
        }
    }

    /// Move table selection by delta rows (positive = down, negative = up)
    fn move_selection(&mut self, delta: i32) {
        let entry_count = self.active_entry_count();
        let new_row = if delta >= 0 {
            self.selected_row.saturating_add(delta as usize)
        } else {
            self.selected_row.saturating_sub((-delta) as usize)
        };
        self.selected_row = new_row.min(entry_count.saturating_sub(1));
        self.update_selection_from_row();
        self.ensure_selection_visible();
    }

    fn active_entry_count(&self) -> usize {
        match self.view_mode {
            ViewMode::Cpu => self.cached_entries.len(),
            ViewMode::Memory => self.cached_heap_entries.len(),
        }
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
            if y == self.table_area.y + 1 {
                self.handle_table_header_click(x);
                return;
            }

            // Calculate row index from click position
            // Table has: border (1) + header (1) = 2 rows before data
            let table_header_offset = 2u16;
            let click_row = y
                .saturating_sub(self.table_area.y)
                .saturating_sub(table_header_offset);

            // Convert visual row to actual entry index using current scroll offset
            let clicked_index = self.scroll_offset + click_row as usize;

            let entry_count = self.active_entry_count();

            if clicked_index < entry_count {
                self.selected_row = clicked_index;
                self.update_selection_from_row();
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

    fn is_double_click(&mut self, x: u16, y: u16) -> bool {
        let now = Instant::now();
        let is_double = self
            .last_click
            .map(|(last_time, last_x, last_y)| {
                now.duration_since(last_time) <= Duration::from_millis(400)
                    && last_x == x
                    && last_y == y
            })
            .unwrap_or(false);
        self.last_click = Some((now, x, y));
        if is_double {
            self.last_click = None;
        }
        is_double
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

    pub fn cpu_sparklines(&self) -> &HashMap<i64, VecDeque<i64>> {
        &self.cached_cpu_sparklines
    }

    pub fn heap_sparklines(&self) -> &HashMap<i64, VecDeque<i64>> {
        &self.cached_heap_sparklines
    }

    pub fn func_history(&self) -> &[(f64, f64)] {
        &self.func_history
    }

    pub fn selected_func(&self) -> Option<&str> {
        self.selected_func_name.as_deref()
    }

    pub fn active_sort(&self) -> TableSort {
        match self.view_mode {
            ViewMode::Cpu => self.cpu_sort,
            ViewMode::Memory => self.heap_sort,
        }
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

    fn update_selected_cpu(&mut self, location_id: i64, func_name: &str) {
        let location_changed = self.selected_location_id != Some(location_id);
        if location_changed {
            self.selected_location_id = Some(location_id);
            self.selected_func_name = Some(func_name.to_string());
            self.chart_data_cache.location_id = None;
        }
    }

    fn update_selected_heap(&mut self, location_id: i64) {
        let location_changed = self.selected_heap_location_id != Some(location_id);
        if location_changed {
            self.selected_heap_location_id = Some(location_id);
            self.heap_chart_cache.location_id = None;
        }
    }

    fn update_selection_from_row(&mut self) {
        match self.view_mode {
            ViewMode::Cpu => {
                let entry = self
                    .cached_entries
                    .get(self.selected_row)
                    .map(|e| (e.location_id, e.function.clone()));
                if let Some((location_id, func_name)) = entry {
                    self.update_selected_cpu(location_id, &func_name);
                }
            }
            ViewMode::Memory => {
                let entry = self
                    .cached_heap_entries
                    .get(self.selected_row)
                    .map(|e| e.location_id);
                if let Some(location_id) = entry {
                    self.update_selected_heap(location_id);
                }
            }
        }
    }

    /// Update sparklines - called once per checkpoint
    /// Updates both CPU and heap sparklines separately
    fn update_sparklines(&mut self) {
        const SPARKLINE_WIDTH: usize = 12;

        // Update CPU sparklines
        let cpu_current: HashMap<i64, i64> = self
            .cached_entries
            .iter()
            .map(|e| (e.location_id, (e.instant_percent * 1000.0) as i64))
            .collect();

        // Drop stale sparklines for locations no longer in the top list
        self.cached_cpu_sparklines
            .retain(|loc_id, _| cpu_current.contains_key(loc_id));

        for (loc_id, sparkline) in self.cached_cpu_sparklines.iter_mut() {
            if sparkline.len() >= SPARKLINE_WIDTH {
                sparkline.pop_front();
            }
            sparkline.push_back(cpu_current.get(loc_id).copied().unwrap_or(0));
        }

        for entry in &self.cached_entries {
            self.cached_cpu_sparklines
                .entry(entry.location_id)
                .or_insert_with(|| {
                    let mut sparkline = VecDeque::with_capacity(SPARKLINE_WIDTH);
                    sparkline.push_back((entry.instant_percent * 1000.0) as i64);
                    sparkline
                });
        }

        // Drop stale CPU entries once they fall off the sparkline window.
        self.prune_cpu_entries();

        // Update heap sparklines
        let heap_current: HashMap<i64, i64> = self
            .cached_heap_entries
            .iter()
            .map(|e| (e.location_id, e.live_bytes))
            .collect();

        // Drop stale sparklines for locations no longer in the top list
        self.cached_heap_sparklines
            .retain(|loc_id, _| heap_current.contains_key(loc_id));

        for (loc_id, sparkline) in self.cached_heap_sparklines.iter_mut() {
            if sparkline.len() >= SPARKLINE_WIDTH {
                sparkline.pop_front();
            }
            sparkline.push_back(heap_current.get(loc_id).copied().unwrap_or(0));
        }

        for entry in &self.cached_heap_entries {
            self.cached_heap_sparklines
                .entry(entry.location_id)
                .or_insert_with(|| {
                    let mut sparkline = VecDeque::with_capacity(SPARKLINE_WIDTH);
                    sparkline.push_back(entry.live_bytes);
                    sparkline
                });
        }

        // Drop stale heap entries once they fall off the sparkline window.
        self.prune_heap_entries();

        // New heap data arrived; force chart cache refresh for live updates.
        self.heap_chart_cache.location_id = None;
    }

    fn refresh_cpu_entries(&mut self) {
        let total_samples = self.total_samples as f64;
        if total_samples <= 0.0 {
            self.cached_entries.clear();
            self.live_cpu_instant.clear();
            return;
        }

        let instant_total: u64 = self.live_cpu_instant.values().sum();
        let mut entries = Vec::new();
        for (&location_id, &total) in &self.live_cpu_totals {
            let info = self.location_info.get(&location_id);
            let (file, line, function) = if let Some(info) = info {
                (info.file.clone(), info.line, info.function.clone())
            } else {
                ("[unknown]".to_string(), 0, "[unknown]".to_string())
            };

            let instant = self
                .live_cpu_instant
                .get(&location_id)
                .copied()
                .unwrap_or(0);
            entries.push(CpuEntry {
                location_id,
                file,
                line,
                function,
                total_samples: total,
                total_percent: (total as f64 / total_samples) * 100.0,
                instant_percent: if instant_total > 0 {
                    (instant as f64 / instant_total as f64) * 100.0
                } else {
                    0.0
                },
            });
        }

        entries.sort_by(|a, b| {
            b.total_samples
                .cmp(&a.total_samples)
                .then(a.location_id.cmp(&b.location_id))
        });
        self.cached_entries = entries;
        for entry in &self.cached_entries {
            self.cpu_last_seen
                .insert(entry.location_id, self.chart_checkpoint_seq);
        }
        self.live_cpu_instant.clear();

        self.sort_cpu_entries();
    }

    fn update_heap_entries(&mut self, entries: Vec<HeapEntry>) {
        self.cached_heap_entries = entries;
        self.sort_heap_entries();
        for entry in &self.cached_heap_entries {
            self.heap_last_seen
                .insert(entry.location_id, self.chart_checkpoint_seq);
        }
    }

    fn prune_cpu_entries(&mut self) {
        let cutoff = self.chart_checkpoint_seq.saturating_sub(SPARKLINE_WIDTH);
        self.cached_entries.retain(|entry| {
            self.cpu_last_seen
                .get(&entry.location_id)
                .copied()
                .unwrap_or(0)
                > cutoff
        });
        let keep: std::collections::HashSet<i64> =
            self.cached_entries.iter().map(|e| e.location_id).collect();
        self.live_cpu_totals.retain(|id, _| keep.contains(id));
        self.location_info.retain(|id, _| keep.contains(id));
        self.cpu_last_seen.retain(|id, _| keep.contains(id));
    }

    fn prune_heap_entries(&mut self) {
        let cutoff = self.chart_checkpoint_seq.saturating_sub(SPARKLINE_WIDTH);
        self.cached_heap_entries.retain(|entry| {
            self.heap_last_seen
                .get(&entry.location_id)
                .copied()
                .unwrap_or(0)
                > cutoff
        });
        let keep: std::collections::HashSet<i64> = self
            .cached_heap_entries
            .iter()
            .map(|e| e.location_id)
            .collect();
        self.heap_live_entries.retain(|id, _| keep.contains(id));
        self.heap_last_seen.retain(|id, _| keep.contains(id));
    }

    fn sort_all_entries(&mut self) {
        self.sort_cpu_entries();
        self.sort_heap_entries();
    }

    fn sort_cpu_entries(&mut self) {
        let sort = self.cpu_sort;
        self.cached_entries.sort_by(|a, b| {
            let ordering = match sort.column {
                SortColumn::Total => cmp_f64(a.total_percent, b.total_percent),
                SortColumn::Live | SortColumn::Trend => {
                    cmp_f64(a.instant_percent, b.instant_percent)
                }
                SortColumn::Function => a.function.cmp(&b.function),
                SortColumn::Location => a.file.cmp(&b.file).then(a.line.cmp(&b.line)),
            };
            let ordering = if sort.descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then(a.location_id.cmp(&b.location_id))
        });
    }

    fn sort_heap_entries(&mut self) {
        let sort = self.heap_sort;
        self.cached_heap_entries.sort_by(|a, b| {
            let ordering = match sort.column {
                SortColumn::Total => a.total_alloc_bytes.cmp(&b.total_alloc_bytes),
                SortColumn::Live | SortColumn::Trend => a.live_bytes.cmp(&b.live_bytes),
                SortColumn::Function => a.function.cmp(&b.function),
                SortColumn::Location => a.file.cmp(&b.file).then(a.line.cmp(&b.line)),
            };
            let ordering = if sort.descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then(a.location_id.cmp(&b.location_id))
        });
    }

    fn toggle_sort(&mut self, column: SortColumn) {
        self.ensure_selection_anchor();

        let sort = match self.view_mode {
            ViewMode::Cpu => &mut self.cpu_sort,
            ViewMode::Memory => &mut self.heap_sort,
        };

        if sort.column == column {
            sort.descending = !sort.descending;
        } else {
            sort.column = column;
            sort.descending = match column {
                SortColumn::Function | SortColumn::Location => false,
                SortColumn::Total | SortColumn::Live | SortColumn::Trend => true,
            };
        }

        match self.view_mode {
            ViewMode::Cpu => self.sort_cpu_entries(),
            ViewMode::Memory => self.sort_heap_entries(),
        }

        self.reselect_anchor();
        self.ensure_selection_visible();
    }

    fn ensure_selection_anchor(&mut self) {
        match self.view_mode {
            ViewMode::Cpu => {
                if self.selected_location_id.is_none()
                    && let Some(entry) = self.cached_entries.get(self.selected_row)
                {
                    self.selected_location_id = Some(entry.location_id);
                }
            }
            ViewMode::Memory => {
                if self.selected_heap_location_id.is_none()
                    && let Some(entry) = self.cached_heap_entries.get(self.selected_row)
                {
                    self.selected_heap_location_id = Some(entry.location_id);
                }
            }
        }
    }

    fn reselect_anchor(&mut self) {
        match self.view_mode {
            ViewMode::Cpu => {
                if let Some(loc_id) = self.selected_location_id
                    && let Some(idx) = self
                        .cached_entries
                        .iter()
                        .position(|e| e.location_id == loc_id)
                {
                    self.selected_row = idx;
                }
            }
            ViewMode::Memory => {
                if let Some(loc_id) = self.selected_heap_location_id
                    && let Some(idx) = self
                        .cached_heap_entries
                        .iter()
                        .position(|e| e.location_id == loc_id)
                {
                    self.selected_row = idx;
                }
            }
        }
    }

    fn table_column_at(&self, x: u16) -> Option<SortColumn> {
        let inner_width = self.table_area.width.saturating_sub(2);
        if inner_width == 0 {
            return None;
        }
        let inner_x = self.table_area.x.saturating_add(1);
        if x < inner_x || x >= inner_x + inner_width {
            return None;
        }

        let fixed_width = 8 + 8 + 14;
        let remaining = inner_width.saturating_sub(fixed_width);
        let func_width = remaining / 2;
        let loc_width = remaining - func_width;

        let mut offset = 0u16;
        let pos = x.saturating_sub(inner_x);

        if pos < offset + 8 {
            return Some(SortColumn::Total);
        }
        offset += 8;
        if pos < offset + 8 {
            return Some(SortColumn::Live);
        }
        offset += 8;
        if pos < offset + func_width {
            return Some(SortColumn::Function);
        }
        offset += func_width;
        if pos < offset + loc_width {
            return Some(SortColumn::Location);
        }
        offset += loc_width;
        if pos < offset + 14 {
            return Some(SortColumn::Trend);
        }
        None
    }

    fn handle_table_header_click(&mut self, x: u16) {
        if let Some(column) = self.table_column_at(x) {
            self.toggle_sort(column);
        }
    }

    /// Query chart data with DB-level aggregation and caching
    /// Returns data aggregated to match the chart's screen columns
    /// Prefetches 3x the visible window for smooth scrolling
    pub fn query_chart_data(
        &mut self,
        visible_start: f64,
        visible_end: f64,
        num_columns: usize,
    ) -> &[(f64, f64)] {
        let location_id = match self.selected_location_id {
            Some(id) => id,
            None => return &[],
        };

        let (prefetch_start, prefetch_end, num_buckets, points_per_sec) =
            self.chart_bucket_params(visible_start, visible_end, num_columns);

        // Check if cache is valid:
        // - Same location
        // - Visible range is within cached range
        // - Resolution is similar (within 20%)
        let cache_valid = self.chart_data_cache.location_id == Some(location_id)
            && visible_start >= self.chart_data_cache.cache_start_secs
            && visible_end <= self.chart_data_cache.cache_end_secs
            && self.chart_data_cache.checkpoint_seq == self.chart_checkpoint_seq
            && (self.chart_data_cache.points_per_sec - points_per_sec).abs()
                / points_per_sec.max(0.001)
                < 0.2;

        if !cache_valid {
            let start_ms = (prefetch_start * 1000.0) as i64;
            let end_ms = (prefetch_end * 1000.0) as i64;

            // Query from DB with aggregation
            let data = if let Some(storage) = &self.storage {
                storage.query_location_timeseries_aggregated(
                    location_id,
                    start_ms,
                    end_ms,
                    num_buckets,
                )
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
            self.chart_data_cache.checkpoint_seq = self.chart_checkpoint_seq;
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
        } else if let Some(elapsed) = self.paused_elapsed {
            elapsed
        } else {
            self.start_time.elapsed()
        }
    }

    /// Get elapsed seconds as f64
    pub fn elapsed_secs(&self) -> f64 {
        if self.is_static() {
            self.static_duration_secs
        } else if let Some(elapsed) = self.paused_elapsed {
            elapsed.as_secs_f64()
        } else {
            self.start_time.elapsed().as_secs_f64()
        }
    }

    /// Get number of entries for scroll bounds
    pub fn entry_count(&self) -> usize {
        self.cached_entries.len()
    }

    /// Get number of heap entries for scroll bounds (Memory mode)
    pub fn heap_entry_count(&self) -> usize {
        self.cached_heap_entries.len()
    }

    /// Get the currently selected heap entry's location_id (for Memory mode)
    pub fn selected_heap_location_id(&self) -> Option<i64> {
        if let Some(loc_id) = self.selected_heap_location_id {
            return Some(loc_id);
        }
        let selected = self
            .selected_row
            .min(self.cached_heap_entries.len().saturating_sub(1));
        self.cached_heap_entries
            .get(selected)
            .map(|e| e.location_id)
    }

    /// Get the currently selected heap entry's function name (for Memory mode)
    pub fn selected_heap_func(&self) -> Option<&str> {
        if let Some(loc_id) = self.selected_heap_location_id {
            return self
                .cached_heap_entries
                .iter()
                .find(|e| e.location_id == loc_id)
                .map(|e| e.function.as_str());
        }
        let selected = self
            .selected_row
            .min(self.cached_heap_entries.len().saturating_sub(1));
        self.cached_heap_entries
            .get(selected)
            .map(|e| e.function.as_str())
    }

    /// Query heap chart data with DB-level aggregation and caching
    /// Returns data aggregated to match the chart's screen columns
    pub fn query_heap_chart_data(
        &mut self,
        visible_start: f64,
        visible_end: f64,
        num_columns: usize,
    ) -> &[(f64, f64)] {
        let location_id = match self.selected_heap_location_id() {
            Some(id) => id,
            None => return &[],
        };

        let (prefetch_start, prefetch_end, num_buckets, points_per_sec) =
            self.chart_bucket_params(visible_start, visible_end, num_columns);

        // Check if cache is valid
        let cache_valid = self.heap_chart_cache.location_id == Some(location_id)
            && visible_start >= self.heap_chart_cache.cache_start_secs
            && visible_end <= self.heap_chart_cache.cache_end_secs
            && self.heap_chart_cache.checkpoint_seq == self.chart_checkpoint_seq
            && (self.heap_chart_cache.points_per_sec - points_per_sec).abs()
                / points_per_sec.max(0.001)
                < 0.2;

        if !cache_valid {
            let start_ms = (prefetch_start * 1000.0) as i64;
            let end_ms = (prefetch_end * 1000.0) as i64;

            // Query from DB with aggregation
            let data = if let Some(storage) = &self.storage {
                storage.query_heap_timeseries_aggregated(location_id, start_ms, end_ms, num_buckets)
            } else if let Some(conn) = &self.conn {
                crate::storage::query_heap_timeseries_aggregated(
                    conn,
                    location_id,
                    start_ms,
                    end_ms,
                    num_buckets,
                )
            } else {
                Vec::new()
            };

            // Update cache
            self.heap_chart_cache.location_id = Some(location_id);
            self.heap_chart_cache.cache_start_secs = prefetch_start;
            self.heap_chart_cache.cache_end_secs = prefetch_end;
            self.heap_chart_cache.points_per_sec = points_per_sec;
            self.heap_chart_cache.data = data;
            self.heap_chart_cache.checkpoint_seq = self.chart_checkpoint_seq;
        }

        &self.heap_chart_cache.data
    }

    fn chart_bucket_params(
        &self,
        visible_start: f64,
        visible_end: f64,
        num_columns: usize,
    ) -> (f64, f64, usize, f64) {
        let visible_range = (visible_end - visible_start).max(0.0);
        if visible_range == 0.0 {
            return (visible_start, visible_end, 0, 0.0);
        }

        if let Some(bucket_secs) = self.chart_state.aggregation_bucket() {
            let bucket_ms = (bucket_secs * 1000.0).round() as i64;
            let bucket_ms = bucket_ms.max(1);
            let visible_start_ms = (visible_start * 1000.0).floor() as i64;
            let visible_end_ms = (visible_end * 1000.0).ceil() as i64;

            let align_down = |value: i64| value - (value % bucket_ms);
            let align_up = |value: i64| {
                let rem = value % bucket_ms;
                if rem == 0 {
                    value
                } else {
                    value + (bucket_ms - rem)
                }
            };

            let aligned_start_ms = align_down(visible_start_ms).max(0);
            let aligned_end_ms = align_up(visible_end_ms).max(aligned_start_ms + bucket_ms);
            let aligned_range_ms = (aligned_end_ms - aligned_start_ms) as f64;

            let prefetch_start_ms =
                align_down(((aligned_start_ms as f64 - aligned_range_ms).max(0.0)) as i64);
            let prefetch_end_ms = align_up(aligned_end_ms + aligned_range_ms as i64);

            let num_buckets = ((prefetch_end_ms - prefetch_start_ms) / bucket_ms).max(1) as usize;
            let points_per_sec = 1.0 / bucket_secs.max(0.001);

            (
                prefetch_start_ms as f64 / 1000.0,
                prefetch_end_ms as f64 / 1000.0,
                num_buckets,
                points_per_sec,
            )
        } else {
            let points_per_sec = num_columns as f64 / visible_range;
            let prefetch_start = (visible_start - visible_range).max(0.0);
            let prefetch_end = visible_end + visible_range;
            let prefetch_range = prefetch_end - prefetch_start;
            let num_buckets =
                ((prefetch_range / visible_range) * num_columns as f64).ceil() as usize;

            (
                prefetch_start,
                prefetch_end,
                num_buckets.max(1),
                points_per_sec,
            )
        }
    }
}

fn cmp_f64(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

fn resolve_internal_stack(
    stack: &[u64],
    resolver: &crate::symbols::SymbolResolver,
) -> crate::symbols::Location {
    for &addr in stack {
        if addr == 0 {
            continue;
        }
        let loc = resolver.resolve(addr);
        if loc.function != "_fini" && loc.function != "[unknown]" {
            return loc;
        }
    }
    crate::symbols::Location::unknown()
}
