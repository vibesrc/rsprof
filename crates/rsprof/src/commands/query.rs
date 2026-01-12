use crate::error::Result;
use rusqlite::Connection;
use std::path::Path;

pub fn run(file: &Path, sql: &str) -> Result<()> {
    let conn = Connection::open(file)?;
    let mut stmt = conn.prepare(sql)?;

    let column_count = stmt.column_count();
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    // Print header
    println!("{}", column_names.join("\t"));

    // Execute and print rows
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let values: Vec<String> = (0..column_count)
            .map(|i| {
                row.get::<_, rusqlite::types::Value>(i)
                    .map(|v| format_value(&v))
                    .unwrap_or_else(|_| "NULL".to_string())
            })
            .collect();
        println!("{}", values.join("\t"));
    }

    Ok(())
}

fn format_value(value: &rusqlite::types::Value) -> String {
    match value {
        rusqlite::types::Value::Null => "NULL".to_string(),
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Real(f) => format!("{:.6}", f),
        rusqlite::types::Value::Text(s) => s.clone(),
        rusqlite::types::Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}
