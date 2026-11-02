//! Item names written differently for the same thing. Matching keys
//! ([`crate::history::item_key`]) are exact; a person confirms that two keys mean the same
//! item (an alias), and matching then uses the whole group. To help them find candidates,
//! [`similarity`] scores how alike two items look. It only proposes: nothing is linked
//! without a person confirming it.

use crate::history::half_width;

/// Lowest [`similarity`] worth proposing.
pub const MIN_SIMILARITY: u32 = 400;

/// The keys that mean the same item as `key`: its canonical key and every alias of that
/// canonical (one level; `pairs` are `(alias, canonical)`). Always contains `key` itself.
pub fn key_group(key: &str, pairs: &[(String, String)]) -> Vec<String> {
    let canonical = pairs
        .iter()
        .find(|(a, _)| a == key)
        .map_or(key, |(_, c)| c.as_str());
    let mut group = vec![key.to_string()];
    if canonical != key {
        group.push(canonical.to_string());
    }
    for (a, c) in pairs {
        if c == canonical && !group.contains(a) {
            group.push(a.clone());
        }
    }
    group
}

fn part_number(mpn: &str) -> String {
    half_width(mpn)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

/// The longest run of digits ("6204" in "6204-2Z"): the part of a part number that rarely
/// changes between a maker's variants and a distributor's spelling.
fn digit_core(mpn: &str) -> &str {
    mpn.split(|c: char| !c.is_ascii_digit())
        .max_by_key(|s| s.len())
        .unwrap_or("")
}

fn bigrams(text: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = half_width(text)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    let mut grams: Vec<(char, char)> = chars.windows(2).map(|w| (w[0], w[1])).collect();
    grams.sort_unstable();
    grams.dedup();
    grams
}

/// Character-bigram Jaccard index of the two texts, in thousandths.
fn text_similarity(a: &str, b: &str) -> u32 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return 0;
    }
    let shared = x.iter().filter(|g| y.binary_search(g).is_ok()).count();
    let union = x.len() + y.len() - shared;
    u32::try_from(shared * 1000 / union).unwrap_or(0)
}

/// How alike two items look, 0–1000: the better of part-number likeness (shared prefix of
/// the normalised part numbers, or the same digit core of at least 3 digits) and
/// description likeness (character bigrams, so it works for Japanese without word
/// boundaries). Deterministic; a proposal only.
pub fn similarity(a_mpn: &str, a_desc: &str, b_mpn: &str, b_desc: &str) -> u32 {
    let (a, b) = (part_number(a_mpn), part_number(b_mpn));
    let mut best = 0;
    if !a.is_empty() && !b.is_empty() {
        let prefix = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
        let longest = a.chars().count().max(b.chars().count());
        if prefix >= 4 {
            best = u32::try_from(prefix * 1000 / longest).unwrap_or(0);
        }
        let (ca, cb) = (digit_core(&a), digit_core(&b));
        if ca.len() >= 3 && ca == cb {
            best = best.max(600);
        }
    }
    // Descriptions often carry the part number too ("ベアリング 6204ZZ").
    let text = text_similarity(&format!("{a_desc} {a_mpn}"), &format!("{b_desc} {b_mpn}"));
    best.max(text).min(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, c)| (a.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn a_key_brings_its_whole_group() {
        let p = pairs(&[("m:62042Z", "m:6204ZZ"), ("d:ベアリング6204", "m:6204ZZ")]);
        let g = key_group("m:62042Z", &p);
        assert_eq!(g, vec!["m:62042Z", "m:6204ZZ", "d:ベアリング6204"]);
        let g = key_group("m:6204ZZ", &p);
        assert_eq!(g, vec!["m:6204ZZ", "m:62042Z", "d:ベアリング6204"]);
        assert_eq!(key_group("m:6205", &p), vec!["m:6205"]);
    }

    #[test]
    fn variants_of_one_part_number_look_alike() {
        let s = similarity("6204-2Z", "", "6204ZZ", "深溝玉軸受");
        assert!(s >= 600, "{s}");
        assert!(s >= MIN_SIMILARITY);
        // Full-width and case do not matter.
        assert_eq!(similarity("ｈｇ-ｋｒ４３", "", "HG-KR43", ""), 1000);
        // Different parts that share only a short prefix are not proposed.
        assert!(similarity("6204ZZ", "", "6305ZZ", "") < MIN_SIMILARITY);
        assert!(similarity("M8-30", "", "M10-30", "") < MIN_SIMILARITY);
    }

    #[test]
    fn descriptions_are_compared_by_characters() {
        let s = similarity("", "六角ボルト M8×30", "", "六角ボルト M8x30 ステンレス");
        assert!(s >= MIN_SIMILARITY, "{s}");
        assert!(similarity("", "六角ボルト M8", "", "深溝玉軸受") < MIN_SIMILARITY);
        assert_eq!(similarity("", "", "", ""), 0);
    }
}
