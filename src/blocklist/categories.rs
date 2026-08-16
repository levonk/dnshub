//! Category bitmap for blocklist entries.
//!
//! Each blocklist domain is tagged with a category bitmap (`u32`) where each
//! bit corresponds to one of the content categories defined in PRD section
//! 4.3. Domains appearing in multiple lists accumulate categories via
//! bitwise OR, enabling the PolicyEngine to make per-category blocking
//! decisions.
//!
//! ## Defined categories (PRD lines 332-350)
//!
//! | Bit | Category      | Description                    |
//! |-----|---------------|--------------------------------|
//! | 0   | ads           | Advertising                    |
//! | 1   | tracker       | Tracking / analytics           |
//! | 2   | telemetry     | Device telemetry / phoning home|
//! | 3   | malware       | Malware distribution / C2      |
//! | 4   | phishing      | Phishing                       |
//! | 5   | adult         | Adult / NSFW content           |
//! | 6   | gambling      | Gambling                       |
//! | 7   | social        | Social media                   |
//! | 8   | dating        | Dating sites                   |
//! | 9   | piracy        | Piracy / copyright infringement|
//! | 10  | streaming     | Video streaming (bandwidth)    |
//! | 11  | games         | Online gaming                  |
//! | 12  | fakenews      | Fake news / misinformation     |
//! | 13  | cryptojacking | Crypto mining scripts          |
//! | 14-31 | reserved    | Future categories              |

/// A category bitmap. Each bit identifies one category (see [`Category`]).
pub type CategoryBitmap = u32;

/// Content categories for blocklist entries.
///
/// The discriminant of each variant is the bit position in the
/// [`CategoryBitmap`]. Variants are ordered to match PRD section 4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Category {
    /// Advertising (bit 0).
    Ads = 0,
    /// Tracking / analytics (bit 1).
    Tracker = 1,
    /// Device telemetry / phoning home (bit 2).
    Telemetry = 2,
    /// Malware distribution / C2 (bit 3).
    Malware = 3,
    /// Phishing (bit 4).
    Phishing = 4,
    /// Adult / NSFW content (bit 5).
    Adult = 5,
    /// Gambling (bit 6).
    Gambling = 6,
    /// Social media (bit 7).
    Social = 7,
    /// Dating sites (bit 8).
    Dating = 8,
    /// Piracy / copyright infringement (bit 9).
    Piracy = 9,
    /// Video streaming / bandwidth (bit 10).
    Streaming = 10,
    /// Online gaming (bit 11).
    Games = 11,
    /// Fake news / misinformation (bit 12).
    FakeNews = 12,
    /// Crypto mining scripts (bit 13).
    Cryptojacking = 13,
}

impl Category {
    /// All defined categories, in bit order.
    pub const ALL: [Category; 14] = [
        Category::Ads,
        Category::Tracker,
        Category::Telemetry,
        Category::Malware,
        Category::Phishing,
        Category::Adult,
        Category::Gambling,
        Category::Social,
        Category::Dating,
        Category::Piracy,
        Category::Streaming,
        Category::Games,
        Category::FakeNews,
        Category::Cryptojacking,
    ];

    /// Returns the bit position of this category (0-13).
    pub const fn bit(self) -> u32 {
        self as u32
    }

    /// Returns the canonical lowercase name for this category.
    pub const fn name(self) -> &'static str {
        match self {
            Category::Ads => "ads",
            Category::Tracker => "tracker",
            Category::Telemetry => "telemetry",
            Category::Malware => "malware",
            Category::Phishing => "phishing",
            Category::Adult => "adult",
            Category::Gambling => "gambling",
            Category::Social => "social",
            Category::Dating => "dating",
            Category::Piracy => "piracy",
            Category::Streaming => "streaming",
            Category::Games => "games",
            Category::FakeNews => "fakenews",
            Category::Cryptojacking => "cryptojacking",
        }
    }
}

/// Convert a [`Category`] into its single-bit [`CategoryBitmap`].
///
/// `category_to_bit(Category::Ads)` returns `1 << 0 == 1`.
pub const fn category_to_bit(cat: Category) -> CategoryBitmap {
    1u32 << cat.bit()
}

/// Look up a [`Category`] by its canonical (lowercase) name.
///
/// Returns `None` for unknown names. Comparison is case-insensitive.
pub fn category_from_str(name: &str) -> Option<Category> {
    let lower = name.trim().to_ascii_lowercase();
    Category::ALL
        .into_iter()
        .find(|c| c.name() == lower.as_str())
}

/// Returns `true` if `bitmap` has the bit for `cat` set.
pub const fn bitmap_has_category(bitmap: CategoryBitmap, cat: Category) -> bool {
    (bitmap & category_to_bit(cat)) != 0
}

/// Build a [`CategoryBitmap`] from an iterator of [`Category`] values by
/// OR-ing each category's bit together.
pub fn bitmap_from_categories<I>(cats: I) -> CategoryBitmap
where
    I: IntoIterator<Item = Category>,
{
    cats.into_iter()
        .map(category_to_bit)
        .fold(0u32, |acc, bit| acc | bit)
}

/// Build a [`CategoryBitmap`] from an iterator of category name strings,
/// ignoring any names that don't map to a known [`Category`].
pub fn bitmap_from_names<'a, I>(names: I) -> CategoryBitmap
where
    I: IntoIterator<Item = &'a str>,
{
    names
        .into_iter()
        .filter_map(category_from_str)
        .map(category_to_bit)
        .fold(0u32, |acc, bit| acc | bit)
}

/// Returns the list of [`Category`] values whose bits are set in `bitmap`,
/// in bit order.
pub fn bitmap_to_categories(bitmap: CategoryBitmap) -> Vec<Category> {
    Category::ALL
        .into_iter()
        .filter(|c| bitmap_has_category(bitmap, *c))
        .collect()
}

/// Returns the list of canonical category names whose bits are set in
/// `bitmap`, in bit order.
pub fn bitmap_to_names(bitmap: CategoryBitmap) -> Vec<&'static str> {
    bitmap_to_categories(bitmap)
        .into_iter()
        .map(Category::name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_categories_defined() {
        // PRD defines 14 categories (bits 0-13).
        assert_eq!(Category::ALL.len(), 14);
        for (i, cat) in Category::ALL.iter().enumerate() {
            assert_eq!(cat.bit(), i as u32, "bit mismatch for {:?}", cat);
        }
    }

    #[test]
    fn test_category_to_bit() {
        assert_eq!(category_to_bit(Category::Ads), 1 << 0);
        assert_eq!(category_to_bit(Category::Tracker), 1 << 1);
        assert_eq!(category_to_bit(Category::Telemetry), 1 << 2);
        assert_eq!(category_to_bit(Category::Malware), 1 << 3);
        assert_eq!(category_to_bit(Category::Phishing), 1 << 4);
        assert_eq!(category_to_bit(Category::Adult), 1 << 5);
        assert_eq!(category_to_bit(Category::Gambling), 1 << 6);
        assert_eq!(category_to_bit(Category::Social), 1 << 7);
        assert_eq!(category_to_bit(Category::Dating), 1 << 8);
        assert_eq!(category_to_bit(Category::Piracy), 1 << 9);
        assert_eq!(category_to_bit(Category::Streaming), 1 << 10);
        assert_eq!(category_to_bit(Category::Games), 1 << 11);
        assert_eq!(category_to_bit(Category::FakeNews), 1 << 12);
        assert_eq!(category_to_bit(Category::Cryptojacking), 1 << 13);
    }

    #[test]
    fn test_category_from_str_roundtrip() {
        for cat in Category::ALL {
            let name = cat.name();
            let parsed = category_from_str(name);
            assert_eq!(parsed, Some(cat), "roundtrip failed for {name}");
        }
    }

    #[test]
    fn test_category_from_str_case_insensitive() {
        assert_eq!(category_from_str("ADS"), Some(Category::Ads));
        assert_eq!(category_from_str("Tracker"), Some(Category::Tracker));
        assert_eq!(category_from_str("  malware  "), Some(Category::Malware));
    }

    #[test]
    fn test_category_from_str_unknown() {
        assert_eq!(category_from_str("unknown"), None);
        assert_eq!(category_from_str(""), None);
        assert_eq!(category_from_str("reserved"), None);
    }

    #[test]
    fn test_bitmap_has_category() {
        let bitmap = category_to_bit(Category::Ads) | category_to_bit(Category::Malware);
        assert!(bitmap_has_category(bitmap, Category::Ads));
        assert!(bitmap_has_category(bitmap, Category::Malware));
        assert!(!bitmap_has_category(bitmap, Category::Tracker));
        assert!(!bitmap_has_category(0, Category::Ads));
    }

    #[test]
    fn test_bitmap_from_categories() {
        let bitmap = bitmap_from_categories([Category::Ads, Category::Tracker, Category::Malware]);
        assert_eq!(bitmap, (1 << 0) | (1 << 1) | (1 << 3));
    }

    #[test]
    fn test_bitmap_from_names() {
        let bitmap = bitmap_from_names(["ads", "tracker", "bogus", "malware"]);
        assert_eq!(bitmap, (1 << 0) | (1 << 1) | (1 << 3));
    }

    #[test]
    fn test_bitmap_from_names_empty() {
        assert_eq!(bitmap_from_names(std::iter::empty()), 0);
    }

    #[test]
    fn test_bitmap_to_categories_roundtrip() {
        let original = [Category::Ads, Category::Social, Category::Cryptojacking];
        let bitmap = bitmap_from_categories(original);
        let decoded = bitmap_to_categories(bitmap);
        assert_eq!(decoded, original.to_vec());
    }

    #[test]
    fn test_bitmap_to_names() {
        let bitmap = (1 << 0) | (1 << 7) | (1 << 13);
        let names = bitmap_to_names(bitmap);
        assert_eq!(names, vec!["ads", "social", "cryptojacking"]);
    }

    #[test]
    fn test_bitmap_to_categories_empty() {
        assert!(bitmap_to_categories(0).is_empty());
    }

    #[test]
    fn test_category_names_match_prd() {
        // Verify canonical names match PRD section 4.3 exactly.
        assert_eq!(Category::Ads.name(), "ads");
        assert_eq!(Category::Tracker.name(), "tracker");
        assert_eq!(Category::Telemetry.name(), "telemetry");
        assert_eq!(Category::Malware.name(), "malware");
        assert_eq!(Category::Phishing.name(), "phishing");
        assert_eq!(Category::Adult.name(), "adult");
        assert_eq!(Category::Gambling.name(), "gambling");
        assert_eq!(Category::Social.name(), "social");
        assert_eq!(Category::Dating.name(), "dating");
        assert_eq!(Category::Piracy.name(), "piracy");
        assert_eq!(Category::Streaming.name(), "streaming");
        assert_eq!(Category::Games.name(), "games");
        assert_eq!(Category::FakeNews.name(), "fakenews");
        assert_eq!(Category::Cryptojacking.name(), "cryptojacking");
    }
}
