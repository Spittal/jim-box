//! Similarity measures shared by track, release and album matching, like
//! beets' `autotag.distance`.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::text::{lead_artist, normalize, strip_noise, tokens};

/// 0 when two names share all their words, 1 when they share none (Dice).
/// Unknown values are neither evidence for nor against.
pub fn string_distance(a: &str, b: &str) -> f64 {
    let (a, b) = (tokens(&strip_noise(a)), tokens(&strip_noise(b)));
    if a.is_empty() || b.is_empty() {
        return 0.5;
    }
    let common = a.iter().filter(|t| b.contains(t)).count().min(b.iter().filter(|t| a.contains(t)).count());
    1.0 - (2 * common) as f64 / (a.len() + b.len()) as f64
}

/// Artists match when one credit contains the other's lead artist, so
/// "Flo Rida" matches "Flo Rida feat. T-Pain".
pub fn artist_distance(a: &str, b: &str) -> f64 {
    let (a, b) = (normalize(a), normalize(b));
    if a.is_empty() || b.is_empty() {
        return 0.5;
    }
    let (lead_a, lead_b) = (normalize(lead_artist(&a)), normalize(lead_artist(&b)));
    if a == b || a.contains(&lead_b) || b.contains(&lead_a) {
        0.0
    } else {
        string_distance(&a, &b)
    }
}

/// The most common non-empty value; ties go to the alphabetically first.
pub fn majority<'a>(values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for value in values.filter(|v| !v.is_empty()) {
        *counts.entry(value).or_default() += 1;
    }
    counts.into_iter().max_by_key(|&(value, count)| (count, std::cmp::Reverse(value))).map(|(v, _)| v)
}

/// Orders release dates earliest first, undated last.
pub fn earlier_date(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_word_overlap() {
        assert_eq!(string_distance("My Girls", "my girls"), 0.0);
        assert_eq!(string_distance("Phantasm feat. Saori Koseki", "phantasm"), 0.0);
        assert_eq!(string_distance("", "anything"), 0.5);
        assert_eq!(string_distance("Peacebone", "My Girls"), 1.0);
    }

    #[test]
    fn matches_artists_with_featured_guests() {
        assert_eq!(artist_distance("Flo Rida", "Flo Rida feat. T‐Pain"), 0.0);
        assert!(artist_distance("Flo Rida", "Pitbull") > 0.9);
    }

    #[test]
    fn finds_the_majority() {
        assert_eq!(majority(["a", "b", "a", ""].into_iter()), Some("a"));
        assert_eq!(majority(["", ""].into_iter()), None);
    }
}
