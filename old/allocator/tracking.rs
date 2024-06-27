use alloc::vec::Vec;
use core::alloc::Layout;
use core::cmp;
use hdrhistogram::serialization::{Serializer, V2Serializer};
use serde::ser::SerializeStruct;
use sync::Mutex;

static ALLOCATIONS_HISTOGRAM: Mutex<Option<Histogram>> = Mutex::new(None);
static DEALLOCATIONS_HISTOGRAM: Mutex<Option<Histogram>> = Mutex::new(None);

pub fn init() {
    ALLOCATIONS_HISTOGRAM.lock().replace(Histogram::new(4096));
    DEALLOCATIONS_HISTOGRAM.lock().replace(Histogram::new(4096));
}

pub fn print_histograms() {
    log::debug!("allocations {}", {
        serde_json::to_string(ALLOCATIONS_HISTOGRAM.lock().as_ref().unwrap()).unwrap()
    });
    log::debug!("deallocations {}", {
        serde_json::to_string(DEALLOCATIONS_HISTOGRAM.lock().as_ref().unwrap()).unwrap()
    });
}

pub fn record_allocation(layout: &Layout) {
    if let Some(mut guard) = ALLOCATIONS_HISTOGRAM.try_lock() {
        if let Some(histogram) = guard.as_mut() {
            histogram.record_allocation(layout.size() as u64);
        }
    }
}

pub fn record_deallocation(layout: &Layout) {
    if let Some(mut guard) = ALLOCATIONS_HISTOGRAM.try_lock() {
        if let Some(histogram) = guard.as_mut() {
            histogram.record_allocation(layout.size() as u64);
        }
    }
}

#[derive(Debug)]
pub struct Histogram {
    pub histogram: hdrhistogram::Histogram<u32>,
    pub max: u64,
    pub outliers: u64,
    pub max_outlier: Option<u64>,
}

impl serde::Serialize for Histogram {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = s.serialize_struct("Histogram", 4)?;

        s.serialize_field("max", &self.max)?;
        s.serialize_field("outliers", &self.outliers)?;
        s.serialize_field("max_outlier", &self.max_outlier)?;

        let mut serializer = V2Serializer::new();
        let mut raw_histogram = Vec::with_capacity(serializer.max_size(&self.histogram).unwrap());
        unsafe { raw_histogram.set_len(serializer.max_size(&self.histogram).unwrap()) }

        serializer
            .serialize_to_buf(&self.histogram, &mut raw_histogram)
            .expect("histogram failed to serialize");

        s.serialize_field("histogram", &raw_histogram)?;
        s.end()
    }
}

impl Histogram {
    pub fn new(max_size: u64) -> Self {
        let histogram = hdrhistogram::Histogram::new_with_max(max_size, 2).unwrap();
        Self {
            histogram,
            max: max_size,
            max_outlier: None,
            outliers: 0,
        }
    }

    pub fn record_allocation(&mut self, mut allocation_size: u64) {
        // clamp the duration to the histogram's max value
        if allocation_size > self.max {
            self.outliers += 1;
            self.max_outlier = cmp::max(self.max_outlier, Some(allocation_size));
            allocation_size = self.max;
        }

        self.histogram
            .record(allocation_size)
            .expect("duration has already been clamped to histogram max value")
    }
}
