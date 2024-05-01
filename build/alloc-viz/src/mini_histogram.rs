use std::fmt::Formatter;
use hdrhistogram::Histogram;
use ratatui::{
    layout::Rect,
    style::Style,
    symbols,
    widgets::{Block, Widget},
};
use serde::{Deserializer};
use serde::de::{Error, SeqAccess};

#[derive(Debug, serde::Deserialize)]
#[allow(unused)]
pub struct SizeHistogram {
    #[serde(deserialize_with = "deserialize_histogram")]
    pub histogram: Histogram<u32>,
    pub max: u64,
    pub outliers: u64,
    pub max_outlier: Option<u64>,
}

fn deserialize_histogram<'de, D>(d: D) -> Result<Histogram<u32>, D::Error> where D: Deserializer<'de> {
    struct V;

    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Histogram<u32>;

        fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
            write!(formatter, "a byte buffer")
        }

        fn visit_bytes<E>(self, mut v: &[u8]) -> Result<Self::Value, E> where E: Error {
            let mut de = hdrhistogram::serialization::Deserializer::new();
            Ok(de.deserialize(&mut v).unwrap())
        }

        // for json
        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error> where A: SeqAccess<'de> {
            let mut vec = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            
            while let Some(e) = seq.next_element::<u8>()? {
                vec.push(e);
            }
            
            self.visit_bytes(&vec)
        }
    }

    d.deserialize_bytes(V)
}

/// This is a Ratatui widget to visualize a latency histogram in a small area.
/// It is based on the [`Sparkline`] widget, so it draws a mini bar chart with
/// some labels for clarity. Unlike Sparkline, it does not omit very small
/// values.
///
/// [`Sparkline`]: ratatui::widgets::Sparkline
pub(crate) struct MiniHistogram<'a> {
    /// A block to wrap the widget in
    block: Option<Block<'a>>,
    /// Widget style
    style: Style,
    /// The histogram data to render
    histogram: &'a SizeHistogram,
    /// The maximum value to take to compute the maximum bar height (if nothing is specified, the
    /// widget uses the max of the dataset)
    max: Option<u64>,
    /// A set of bar symbols used to represent the give data
    bar_set: symbols::bar::Set,
    /// precision for the labels
    precision: usize,
}

#[derive(Debug, Default)]
pub(crate) struct HistogramMetadata {
    /// The max recorded value in the histogram. This is the label for the bottom-right in the chart
    pub(crate) max_value: u64,
    /// The min recorded value in the histogram.
    pub(crate) min_value: u64,
    /// The value of the bucket with the greatest quantity
    pub(crate) max_bucket: u64,
    /// Number of high outliers, if any
    pub(crate) high_outliers: u64,
    pub(crate) highest_outlier: Option<u64>,
}

impl<'a> Widget for MiniHistogram<'a> {
    fn render(mut self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let inner_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };

        if inner_area.height < 1 {
            return;
        }

        let (data, metadata) = chart_data(&self.histogram, inner_area.width - 3);

        let max_qty_label = metadata.max_bucket.to_string();
        let max_record_label = format!("{:.prec$?}", metadata.max_value, prec = self.precision,);
        let min_record_label = format!("{:.prec$?}", metadata.min_value, prec = self.precision,);
        let y_axis_label_width = max_qty_label.len() as u16;

        render_legend(
            inner_area,
            buf,
            &metadata,
            max_record_label,
            min_record_label,
            max_qty_label,
        );

        let legend_height = if metadata.high_outliers > 0 { 2 } else { 1 };

        // Shrink the bars area by 1 row from the bottom
        // and `y_axis_label_width` columns from the left.
        let bars_area = Rect {
            x: inner_area.x + y_axis_label_width,
            y: inner_area.y,
            width: inner_area.width - y_axis_label_width,
            height: inner_area.height - legend_height,
        };
        self.render_bars(bars_area, buf, data);
    }
}

impl<'a> MiniHistogram<'a> {
    pub fn new(histogram: &'a SizeHistogram) -> Self {
        MiniHistogram {
            block: None,
            style: Default::default(),
            histogram,
            max: None,
            bar_set: symbols::bar::NINE_LEVELS,
            precision: 4,
        }
    }

    fn render_bars(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        data: Vec<u64>,
    ) {
        let max = match self.max {
            Some(v) => v,
            None => *data.iter().max().unwrap_or(&1u64),
        };
        let max_index = std::cmp::min(area.width as usize, data.len());
        let mut data = data
            .iter()
            .take(max_index)
            .map(|e| {
                if max != 0 {
                    let r = e * u64::from(area.height) * 8 / max;
                    // This is the only difference in the bar rendering logic
                    // between MiniHistogram and Sparkline. At least render a
                    // ONE_EIGHT, if the value is greater than 0, even if it's
                    // relatively very small.
                    if *e > 0 && r == 0 {
                        1
                    } else {
                        r
                    }
                } else {
                    0
                }
            })
            .collect::<Vec<u64>>();
        for j in (0..area.height).rev() {
            for (i, d) in data.iter_mut().enumerate() {
                let symbol = match *d {
                    0 => self.bar_set.empty,
                    1 => self.bar_set.one_eighth,
                    2 => self.bar_set.one_quarter,
                    3 => self.bar_set.three_eighths,
                    4 => self.bar_set.half,
                    5 => self.bar_set.five_eighths,
                    6 => self.bar_set.three_quarters,
                    7 => self.bar_set.seven_eighths,
                    _ => self.bar_set.full,
                };
                buf.get_mut(area.left() + i as u16, area.top() + j)
                    .set_symbol(symbol)
                    .set_style(self.style);

                if *d > 8 {
                    *d -= 8;
                } else {
                    *d = 0;
                }
            }
        }
    }

    pub fn precision(mut self, precision: usize) -> MiniHistogram<'a> {
        self.precision = precision;
        self
    }

    // The same Sparkline setter methods below

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> MiniHistogram<'a> {
        self.block = Some(block);
        self
    }

    #[allow(dead_code)]
    pub fn style(mut self, style: Style) -> MiniHistogram<'a> {
        self.style = style;
        self
    }

    #[allow(dead_code)]
    pub fn max(mut self, max: u64) -> MiniHistogram<'a> {
        self.max = Some(max);
        self
    }

    #[allow(dead_code)]
    pub fn bar_set(mut self, bar_set: symbols::bar::Set) -> MiniHistogram<'a> {
        self.bar_set = bar_set;
        self
    }
}

fn render_legend(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    metadata: &HistogramMetadata,
    max_record_label: String,
    min_record_label: String,
    max_qty_label: String,
) {
    // If there are outliers, display a note
    let labels_pos = if metadata.high_outliers > 0 {
        let outliers = format!(
            "{} outliers (highest: {:?})",
            metadata.high_outliers,
            metadata
                .highest_outlier
                .expect("if there are outliers, the highest should be set")
        );
        buf.set_string(
            area.right() - outliers.len() as u16,
            area.bottom() - 1,
            &outliers,
            Style::default(),
        );
        2
    } else {
        1
    };

    // top left: max quantity
    buf.set_string(area.left(), area.top(), &max_qty_label, Style::default());
    // bottom left below the chart: min time
    buf.set_string(
        area.left() + max_qty_label.len() as u16,
        area.bottom() - labels_pos,
        &min_record_label,
        Style::default(),
    );
    // bottom right: max time
    buf.set_string(
        area.right() - max_record_label.len() as u16,
        area.bottom() - labels_pos,
        &max_record_label,
        Style::default(),
    );
}

/// From the histogram, build a visual representation by trying to make as
/// many buckets as the width of the render area.
fn chart_data(histogram: &SizeHistogram, width: u16) -> (Vec<u64>, HistogramMetadata) {
    let &SizeHistogram {
        ref histogram,
        outliers,
        max_outlier,
        ..
    } = histogram;

    let step_size = ((histogram.max() - histogram.min()) as f64 / width as f64).ceil() as u64 + 1;
    // `iter_linear` panics if step_size is 0
    let data = if step_size > 0 {
        let mut found_first_nonzero = false;
        let data: Vec<u64> = histogram
            .iter_linear(step_size)
            .filter_map(|value| {
                let count = value.count_since_last_iteration();
                // Remove the 0s from the leading side of the buckets.
                // Because HdrHistogram can return empty buckets depending
                // on its internal state, as it approximates values.
                if count == 0 && !found_first_nonzero {
                    None
                } else {
                    found_first_nonzero = true;
                    Some(count)
                }
            })
            .collect();
        data
    } else {
        Vec::new()
    };
    let max_bucket = data.iter().max().copied().unwrap_or_default();
    (
        data,
        HistogramMetadata {
            max_value: histogram.max(),
            min_value: histogram.min(),
            max_bucket,
            high_outliers: outliers,
            highest_outlier: max_outlier,
        },
    )
}
