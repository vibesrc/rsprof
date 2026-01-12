use crate::error::Result;
use crate::tui::App;
use std::path::Path;

/// Run the view command - opens a profile in the unified TUI
pub fn run(file: &Path) -> Result<()> {
    let mut app = App::from_file(file)?;
    app.run()?;
    Ok(())
}
