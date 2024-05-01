mod mini_histogram;
mod size;

use crate::mini_histogram::SizeHistogram;
use crate::size::Sizes;
use crossterm::event::{KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{event, ExecutableCommand};
use ratatui::prelude::*;
use ratatui::Terminal;
use std::env::args;
use std::fs;
use std::io::{stdout, BufReader, Result};

fn main() -> Result<()> {
    let file = fs::File::open(args().nth(1).expect("expected path as first argument"))?;
    let reader = BufReader::new(file);
    let histogram: SizeHistogram = serde_json::from_reader(reader)?;

    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    loop {
        terminal.draw(|frame| {
            let area = frame.size();
            frame.render_widget(
                Sizes::new(&histogram)
                    .histogram_title("Allocation Sizes (press q to quit)")
                    .percentiles_title("Alloc Sizes Percentiles"),
                area,
            );
        })?;

        if event::poll(std::time::Duration::from_millis(16))? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
