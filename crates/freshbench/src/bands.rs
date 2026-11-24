//! Function-size bands and size-normalized GED.
//!
//! GED grows with the size of the source CFG, so totals are dominated by a few
//! large functions. Every table is also reported per band of source CFG nodes.

/// (label, lowest node count, highest node count), inclusive.
pub const BANDS: [(&str, u32, u32); 5] = [
    ("<6", 0, 5),
    ("6-20", 6, 20),
    ("21-50", 21, 50),
    ("51-100", 51, 100),
    (">100", 101, u32::MAX),
];

pub const UNKNOWN: &str = "unknown";

pub fn band(source_nodes: Option<u32>) -> &'static str {
    match source_nodes {
        None => UNKNOWN,
        Some(n) => BANDS
            .iter()
            .find(|(_, lo, hi)| *lo <= n && n <= *hi)
            .map(|b| b.0)
            .unwrap_or(UNKNOWN),
    }
}

pub fn labels() -> impl Iterator<Item = &'static str> {
    BANDS.iter().map(|b| b.0)
}

/// GED / (source nodes + source edges); None when a side is missing or the size is 0.
pub fn normalized_ged(
    ged: Option<f64>,
    source_nodes: Option<u32>,
    source_edges: Option<u32>,
) -> Option<f64> {
    let size = source_nodes? + source_edges?;
    if size == 0 {
        return None;
    }
    Some(ged? / f64::from(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges() {
        let got: Vec<_> = [0, 5, 6, 20, 21, 50, 51, 100, 101, 10_000]
            .iter()
            .map(|n| band(Some(*n)))
            .collect();
        assert_eq!(
            got,
            ["<6", "<6", "6-20", "6-20", "21-50", "21-50", "51-100", "51-100", ">100", ">100"]
        );
        assert_eq!(band(None), "unknown");
    }

    #[test]
    fn normalized() {
        assert_eq!(normalized_ged(Some(0.0), Some(5), Some(6)), Some(0.0));
        assert_eq!(normalized_ged(Some(11.0), Some(5), Some(6)), Some(1.0));
        assert_eq!(normalized_ged(None, Some(5), Some(6)), None);
        assert_eq!(normalized_ged(Some(3.0), Some(0), Some(0)), None);
    }
}
