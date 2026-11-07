// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Lowercase, strip accents/diacritics (so Greek tonos and Latin accents don't matter)
/// and fold the final sigma, for case- and accent-insensitive search.
pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ς' { 'σ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn folds_greek_and_latin() {
        assert_eq!(fold("Λάμψη"), "λαμψη");
        assert_eq!(fold("ΛΑΜΨΗ"), "λαμψη");
        assert_eq!(fold("Μελωδίας"), fold("ΜΕΛΩΔΙΑΣ"));
        assert_eq!(fold("Beyoncé"), "beyonce");
    }
}
