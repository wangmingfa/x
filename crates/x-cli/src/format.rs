//! Output rendering shared by every command.
//!
//! Three formats, one code path: aligned table for humans, tab separated
//! plain text for scripts, and JSON for machines. Commands never print
//! directly, they build rows and hand them to a [`Renderer`], which keeps the
//! format decision in exactly one place.

use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthStr;

/// How command results are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum OutputFormat {
    /// Aligned columns with a header. Default.
    #[default]
    Table,
    /// One record per line, fields separated by a single tab, no header.
    Plain,
    /// Pretty printed JSON.
    Json,
    /// One JSON object per row, newline delimited — stream friendly.
    Jsonl,
    /// RFC 4180 style CSV with a header row.
    Csv,
}

/// One table cell.
///
/// A dedicated newtype lets a row mix literals and owned values
/// (`row!["node", process.name.clone()]`) without forcing the caller to decide
/// on `&str` or `String` up front.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cell(String);

impl Cell {
    /// The cell text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Take ownership of the text.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! from_number {
    ($($ty:ty),*) => {
        $(impl From<$ty> for Cell {
            fn from(value: $ty) -> Self {
                Cell(value.to_string())
            }
        })*
    };
}

from_number!(u8, u16, u32, u64, usize, i32, f32, f64);

impl From<&str> for Cell {
    fn from(value: &str) -> Self {
        Cell(value.to_string())
    }
}

impl From<&String> for Cell {
    fn from(value: &String) -> Self {
        Cell(value.clone())
    }
}

impl From<String> for Cell {
    fn from(value: String) -> Self {
        Cell(value)
    }
}

/// Shorten `text` to `max` display columns, marking the cut with an ellipsis.
///
/// Long command lines would otherwise dominate a table, so the human format
/// clips them while `--json` keeps the full value.
pub fn clip(text: &str, max: usize) -> String {
    if text.width() <= max || max < 2 {
        return text.to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = ch.to_string().width();
        if width + ch_width > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('\u{2026}');
    out
}

/// Build a row of [`Cell`]s, accepting a mix of literals, numbers and owned text.
#[macro_export]
macro_rules! row {
    ($($cell:expr),+ $(,)?) => {
        vec![$($crate::format::Cell::from($cell)),+]
    };
}

/// A table of strings to render.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Start a table with `headers`.
    pub fn new(headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// Append a row. Short rows are padded, long rows are kept as given.
    pub fn push(&mut self, cells: impl IntoIterator<Item = Cell>) -> &mut Self {
        self.rows
            .push(cells.into_iter().map(Cell::into_string).collect());
        self
    }

    /// Number of data rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// `true` when there is no data row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.width()).collect();
        for row in &self.rows {
            for (index, cell) in row.iter().enumerate() {
                let width = cell.width();
                match widths.get_mut(index) {
                    Some(current) => *current = (*current).max(width),
                    None => widths.push(width),
                }
            }
        }
        widths
    }

    fn render_table(&self, out: &mut impl Write, color: bool) -> io::Result<()> {
        let widths = self.widths();
        let write_row = |out: &mut dyn Write, cells: &[String]| -> io::Result<()> {
            let last = cells.len().saturating_sub(1);
            for (index, cell) in cells.iter().enumerate() {
                if index > 0 {
                    write!(out, "  ")?;
                }
                // The final column is never padded: trailing spaces turn into
                // invisible noise in diffs and in piped output.
                let width = match widths.get(index).copied() {
                    Some(width) if index != last => width,
                    _ => 0,
                };
                write!(out, "{cell:<width$}")?;
            }
            writeln!(out)
        };

        if color {
            let mut header = self.headers.clone();
            if let Some(first) = header.first_mut() {
                *first = format!("\x1b[1m{first}\x1b[0m");
            }
            write_row(out, &header)?;
        } else {
            write_row(out, &self.headers)?;
        }
        for row in &self.rows {
            write_row(out, row)?;
        }
        Ok(())
    }

    fn render_plain(&self, out: &mut impl Write) -> io::Result<()> {
        for row in &self.rows {
            let line = row
                .iter()
                .map(|cell| cell.replace(['\t', '\n'], " "))
                .collect::<Vec<_>>()
                .join("\t");
            writeln!(out, "{line}")?;
        }
        Ok(())
    }
}

/// Render one CSV record: quote fields that contain separators, quotes or
/// newlines, double the quotes inside, join with commas.
fn csv_line(fields: &[String]) -> String {
    fields
        .iter()
        .map(|field| {
            if field.contains(',') || field.contains('"') || field.contains('\n') {
                format!("\"{}\"", field.replace('"', "\"\""))
            } else {
                field.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Writes command results in the selected format.
pub struct Renderer {
    format: OutputFormat,
    color: bool,
    out: Box<dyn Write>,
}

impl Renderer {
    /// A renderer writing to stdout.
    pub fn stdout(format: OutputFormat, color: bool) -> Self {
        Self {
            format,
            color,
            out: Box::new(io::stdout()),
        }
    }

    /// A renderer writing to an arbitrary sink, used by tests.
    pub fn to_sink(format: OutputFormat, color: bool, out: impl Write + 'static) -> Self {
        Self {
            format,
            color,
            out: Box::new(out),
        }
    }

    /// Whether ANSI colors are enabled.
    pub fn color(&self) -> bool {
        self.color
    }

    /// The selected format.
    pub fn format(&self) -> OutputFormat {
        self.format
    }

    /// Write a table.
    pub fn table(&mut self, table: &Table) -> io::Result<()> {
        match self.format {
            OutputFormat::Table => table.render_table(&mut self.out, self.color),
            OutputFormat::Plain => table.render_plain(&mut self.out),
            OutputFormat::Json => {
                let rows: Vec<serde_json::Value> = table
                    .rows
                    .iter()
                    .map(|row| {
                        let mut object = serde_json::Map::new();
                        for (index, header) in table.headers.iter().enumerate() {
                            object.insert(
                                header.clone(),
                                serde_json::Value::String(
                                    row.get(index).cloned().unwrap_or_default(),
                                ),
                            );
                        }
                        serde_json::Value::Object(object)
                    })
                    .collect();
                self.json(&rows)
            }
            OutputFormat::Jsonl => {
                for row in &table.rows {
                    let mut object = serde_json::Map::new();
                    for (index, header) in table.headers.iter().enumerate() {
                        object.insert(
                            header.clone(),
                            serde_json::Value::String(row.get(index).cloned().unwrap_or_default()),
                        );
                    }
                    writeln!(self.out, "{}", serde_json::Value::Object(object))?;
                }
                Ok(())
            }
            OutputFormat::Csv => {
                let header: Vec<String> = table.headers.clone();
                self.out.write_all(csv_line(&header).as_bytes())?;
                self.out.write_all(b"\r\n")?;
                for row in &table.rows {
                    let fields: Vec<String> = (0..table.headers.len())
                        .map(|index| row.get(index).cloned().unwrap_or_default())
                        .collect();
                    self.out.write_all(csv_line(&fields).as_bytes())?;
                    self.out.write_all(b"\r\n")?;
                }
                Ok(())
            }
        }
    }

    /// Write any serializable value as JSON.
    pub fn json(&mut self, value: &impl serde::Serialize) -> io::Result<()> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(self.out, "{text}")
    }

    /// Write a raw value as JSON regardless of the selected format.
    pub fn always_json(&mut self, value: &impl serde::Serialize) -> io::Result<()> {
        self.json(value)
    }

    /// Write a free form line: a hint, a confirmation or a status message.
    pub fn line(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        writeln!(self.out, "{}", text.as_ref())
    }

    /// Flush the underlying sink.
    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Color choice for the process: honor `NO_COLOR`, `--color` and pipe detection.
pub fn should_colorize(explicit: Option<bool>) -> bool {
    match explicit {
        Some(choice) => choice,
        None => std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    }
}

/// A tiny reader abstraction so confirmations are testable.
pub trait Confirmer {
    /// Ask a yes/no question and return the answer.
    fn confirm(&mut self, question: &str) -> io::Result<bool>;
}

/// Real implementation: prompt on stderr when stdin is a terminal.
pub struct StdinConfirmer;

impl Confirmer for StdinConfirmer {
    fn confirm(&mut self, question: &str) -> io::Result<bool> {
        if !io::stdin().is_terminal() {
            // A pipe means a script is driving us: never hang waiting for a
            // human. Callers treat `false` as "the user declined".
            return Ok(false);
        }
        let mut stderr = io::stderr();
        write!(stderr, "{question} [y/N] ")?;
        stderr.flush()?;

        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        Ok(answer == "y" || answer == "yes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Shared, inspectable output buffer.
    #[derive(Clone, Default)]
    struct Buffer(Rc<RefCell<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).expect("utf8")
        }
    }

    fn render(format: OutputFormat, build: impl FnOnce(&mut Table)) -> String {
        let mut table = Table::new(["name", "port"]);
        build(&mut table);
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(format, false, buffer.clone());
        renderer.table(&table).expect("write");
        renderer.flush().expect("flush");
        buffer.text()
    }

    #[test]
    fn table_aligns_columns_by_display_width() {
        let text = render(OutputFormat::Table, |table| {
            table.push(row!["node", "8080"]);
            table.push(row!["nginx-master", "80"]);
        });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "name          port");
        assert_eq!(lines[1], "node          8080");
        assert_eq!(lines[2], "nginx-master  80");
    }

    #[test]
    fn plain_output_has_no_header_and_is_tab_separated() {
        let text = render(OutputFormat::Plain, |table| {
            table.push(row!["node", "8080"]);
        });
        assert_eq!(text, "node\t8080\n");
    }

    #[test]
    fn json_output_uses_header_names_as_keys() {
        let text = render(OutputFormat::Json, |table| {
            table.push(row!["node", "8080"]);
        });
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value[0]["name"], "node");
        assert_eq!(value[0]["port"], "8080");
    }

    #[test]
    fn jsonl_output_is_one_object_per_row() {
        let text = render(OutputFormat::Jsonl, |table| {
            table.push(row!["node", "8080"]);
            table.push(row!["nginx", "80"]);
        });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one object per row, no header: {text}");
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("jsonl line 1");
        assert_eq!(first["name"], "node");
        assert_eq!(first["port"], "8080");
        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("jsonl line 2");
        assert_eq!(second["name"], "nginx");
    }

    #[test]
    fn csv_output_has_a_header_and_quotes_specials() {
        let text = render(OutputFormat::Csv, |table| {
            table.push(row!["node", "8080"]);
            table.push(row!["a,b", "plain"]);
        });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "name,port");
        assert_eq!(lines[1], "node,8080");
        assert_eq!(lines[2], "\"a,b\",plain");
    }

    #[test]
    fn cells_containing_tabs_stay_one_field_in_plain_output() {
        let text = render(OutputFormat::Plain, |table| {
            table.push(row!["a\tb", "1"]);
        });
        assert_eq!(text, "a b\t1\n");
    }

    #[test]
    fn short_rows_are_padded_not_dropped() {
        let text = render(OutputFormat::Table, |table| {
            table.push(row!["only"]);
        });
        assert_eq!(text, "name  port\nonly\n");
    }

    #[test]
    fn clip_respects_display_width_and_marks_the_cut() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefgh", 5), "abcd\u{2026}");
        // Wide characters count as two columns: four columns fit exactly.
        assert_eq!(clip("\u{4e2d}\u{6587}", 4), "\u{4e2d}\u{6587}");
        assert_eq!(clip("\u{4e2d}\u{6587}", 3), "\u{4e2d}\u{2026}");
        assert_eq!(clip("anything", 1), "anything");
    }

    #[test]
    fn line_writes_raw_text() {
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Json, false, buffer.clone());
        renderer.line("port 8080 is free").expect("write");
        assert_eq!(buffer.text(), "port 8080 is free\n");
    }
}
