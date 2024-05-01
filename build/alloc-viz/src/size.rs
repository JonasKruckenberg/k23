use crate::mini_histogram::{MiniHistogram, SizeHistogram};
use ratatui::prelude::*;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use std::cmp;

const MIN_HISTOGRAM_BLOCK_WIDTH: u16 = 22;

pub struct Sizes<'a> {
    /// The histogram data to render
    histogram: &'a SizeHistogram,
    /// Title for percentiles block
    percentiles_title: &'a str,
    /// Title for histogram sparkline block
    histogram_title: &'a str,
    /// Fixed width for percentiles block
    percentiles_width: u16,
}

fn border_block<'a>() -> Block<'a> {
    Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
}

impl<'a> Widget for Sizes<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Only split the durations area in half if we're also drawing a
        // sparkline. We require UTF-8 to draw the sparkline and also enough width.
        let percentiles_width = match self.percentiles_width {
            // Fixed width
            width if width > 0 => width,
            // Long enough for the title or for a single line
            // like "p99: 544.77µs" (13) (and borders on the sides).
            _ => cmp::max(self.percentiles_title.len() as u16, 13_u16) + 2,
        };

        // If there isn't enough width left after drawing the percentiles
        // then we won't draw the sparkline at all.
        let (percentiles_area, histogram_area) =
            if area.width < percentiles_width + MIN_HISTOGRAM_BLOCK_WIDTH {
                (area, None)
            } else {
                let areas = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(
                        [
                            Constraint::Length(percentiles_width),
                            Constraint::Min(MIN_HISTOGRAM_BLOCK_WIDTH),
                        ]
                        .as_ref(),
                    )
                    .split(area);
                (areas[0], Some(areas[1]))
            };

        let percentiles_widget = Percentiles::new(&self.histogram).title(self.percentiles_title);
        percentiles_widget.render(percentiles_area, buf);

        if let Some(histogram_area) = histogram_area {
            let histogram_widget = MiniHistogram::new(self.histogram)
                .block(border_block().title(self.histogram_title))
                .precision(2);
            histogram_widget.render(histogram_area, buf);
        }
    }
}

impl<'a> Sizes<'a> {
    pub(crate) fn new(histogram: &'a SizeHistogram) -> Self {
        Self {
            histogram,
            percentiles_title: "Percentiles",
            histogram_title: "Histogram",
            percentiles_width: 0,
        }
    }

    pub(crate) fn percentiles_title(mut self, title: &'a str) -> Self {
        self.percentiles_title = title;
        self
    }

    pub(crate) fn histogram_title(mut self, title: &'a str) -> Self {
        self.histogram_title = title;
        self
    }
}

struct Percentiles<'a> {
    /// The histogram data to render
    histogram: &'a SizeHistogram,
    /// The title of the paragraph
    title: &'a str,
}

impl<'a> Widget for Percentiles<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner =
            Paragraph::new(self.make_percentiles_inner()).block(border_block().title(self.title));

        inner.render(area, buf)
    }
}

impl<'a> Percentiles<'a> {
    pub(crate) fn new(histogram: &'a SizeHistogram) -> Self {
        Self {
            histogram,
            title: "Percentiles",
        }
    }

    pub(crate) fn make_percentiles_inner(&self) -> Text<'static> {
        let mut text = Text::default();

        // Get the important percentile values from the histogram
        let pairs = [10f64, 25f64, 50f64, 75f64, 90f64, 95f64, 99f64]
            .iter()
            .map(move |i| (*i, self.histogram.histogram.value_at_percentile(*i)));
        let percentiles = pairs.map(|pair| Line::from(format!("p{:>2}: {} bytes", pair.0, pair.1)));

        text.extend(percentiles);
        text
    }

    pub(crate) fn title(mut self, title: &'a str) -> Percentiles<'a> {
        self.title = title;
        self
    }
}
