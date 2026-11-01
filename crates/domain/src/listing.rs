//! Supplier product listings (Phase C): who may move a listing between states, and what a
//! listing must contain before Chuo-Sogo reviews it. Deterministic, no I/O.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingStatus {
    Draft,
    Submitted,
    Approved,
    Rejected,
    Retired,
}

impl ListingStatus {
    pub const fn code(self) -> &'static str {
        match self {
            ListingStatus::Draft => "draft",
            ListingStatus::Submitted => "submitted",
            ListingStatus::Approved => "approved",
            ListingStatus::Rejected => "rejected",
            ListingStatus::Retired => "retired",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            ListingStatus::Draft,
            ListingStatus::Submitted,
            ListingStatus::Approved,
            ListingStatus::Rejected,
            ListingStatus::Retired,
        ]
        .into_iter()
        .find(|s| s.code() == code)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// A member of the supplier organisation that owns the listing.
    Supplier,
    /// Chuo-Sogo staff with `moderate_listings`.
    Moderator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Change content (or photos). Takes a submitted, rejected or published listing back to
    /// draft: anything changed is reviewed again before it is shown.
    Edit,
    Submit,
    Approve,
    Reject,
    Retire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: ListingStatus,
    pub action: Action,
}

pub fn next(
    from: ListingStatus,
    action: Action,
    actor: Actor,
) -> Result<ListingStatus, InvalidTransition> {
    use ListingStatus::*;
    let to = match (actor, action, from) {
        (Actor::Supplier, Action::Edit, Draft | Submitted | Rejected | Approved) => Some(Draft),
        (Actor::Supplier, Action::Submit, Draft) => Some(Submitted),
        (Actor::Moderator, Action::Approve, Submitted) => Some(Approved),
        (Actor::Moderator, Action::Reject, Submitted) => Some(Rejected),
        (Actor::Supplier, Action::Retire, Draft | Submitted | Approved | Rejected) => Some(Retired),
        (Actor::Moderator, Action::Retire, Approved) => Some(Retired),
        _ => None,
    };
    to.ok_or(InvalidTransition { from, action })
}

pub const MIN_PHOTOS: usize = 2;
pub const MAX_PHOTOS: usize = 5;
pub const MAX_DOCUMENTS: usize = 5;
/// Shortest acceptable long side of a listing photo, in pixels.
pub const MIN_PHOTO_LONG_SIDE: u32 = 1200;
pub const MAX_SPECS: usize = 50;

/// What the server knows about a listing when the supplier submits it.
#[derive(Debug, Clone, Default)]
pub struct Completeness {
    pub title: bool,
    pub manufacturer: bool,
    pub model_number: bool,
    pub description: bool,
    pub photos: usize,
    pub has_price: bool,
}

/// Missing items, as stable codes; empty = ready for review.
pub fn missing_for_submit(c: &Completeness) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (ok, code) in [
        (c.title, "title"),
        (c.manufacturer, "manufacturer"),
        (c.model_number, "model_number"),
        (c.description, "description"),
        (c.has_price, "price"),
    ] {
        if !ok {
            out.push(code);
        }
    }
    if c.photos < MIN_PHOTOS {
        out.push("photos");
    }
    out
}

/// Largest photo accepted (pixels): bounds the memory a viewer needs to decode it.
pub const MAX_PHOTO_PIXELS: u64 = 50_000_000;

/// A photo is good enough for the catalogue: at least 1200 px on its long side, not an
/// extreme strip (aspect ratio at most 4:1), and at most 50 megapixels.
pub fn photo_acceptable(width: u32, height: u32) -> bool {
    if u64::from(width) * u64::from(height) > MAX_PHOTO_PIXELS {
        return false;
    }
    let (long, short) = if width >= height {
        (width, height)
    } else {
        (height, width)
    };
    long >= MIN_PHOTO_LONG_SIDE && short > 0 && long <= short.saturating_mul(4)
}

/// Model numbers compare without case, spaces or hyphens ("6204-2RS" = "6204 2rs").
pub fn normalise_model(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ListingStatus::*;

    #[test]
    fn suppliers_list_and_chuo_sogo_reviews() {
        assert_eq!(next(Draft, Action::Submit, Actor::Supplier), Ok(Submitted));
        assert_eq!(
            next(Submitted, Action::Approve, Actor::Moderator),
            Ok(Approved)
        );
        assert_eq!(
            next(Submitted, Action::Reject, Actor::Moderator),
            Ok(Rejected)
        );
        // Suppliers can never approve their own listing; moderators never edit content.
        assert!(next(Submitted, Action::Approve, Actor::Supplier).is_err());
        assert!(next(Draft, Action::Edit, Actor::Moderator).is_err());
        assert!(next(Draft, Action::Approve, Actor::Moderator).is_err());
        // Any change to a published listing unpublishes it until reviewed again.
        assert_eq!(next(Approved, Action::Edit, Actor::Supplier), Ok(Draft));
        assert_eq!(next(Rejected, Action::Edit, Actor::Supplier), Ok(Draft));
        // Retired is final.
        for a in [
            Action::Edit,
            Action::Submit,
            Action::Approve,
            Action::Retire,
        ] {
            for actor in [Actor::Supplier, Actor::Moderator] {
                assert!(next(Retired, a, actor).is_err());
            }
        }
        assert_eq!(
            next(Approved, Action::Retire, Actor::Moderator),
            Ok(Retired)
        );
    }

    #[test]
    fn submission_needs_the_essentials_and_two_photos() {
        let full = Completeness {
            title: true,
            manufacturer: true,
            model_number: true,
            description: true,
            photos: 2,
            has_price: true,
        };
        assert!(missing_for_submit(&full).is_empty());
        let mut c = full.clone();
        c.photos = 1;
        c.has_price = false;
        assert_eq!(missing_for_submit(&c), vec!["price", "photos"]);
        assert_eq!(
            missing_for_submit(&Completeness::default()),
            vec![
                "title",
                "manufacturer",
                "model_number",
                "description",
                "price",
                "photos"
            ]
        );
    }

    #[test]
    fn photos_must_be_large_enough() {
        assert!(photo_acceptable(1200, 900));
        assert!(photo_acceptable(900, 1600));
        assert!(!photo_acceptable(1199, 800));
        assert!(!photo_acceptable(4000, 900)); // a strip, not a product photo
        assert!(!photo_acceptable(1200, 0));
        assert!(!photo_acceptable(60_000, 15_000), "decompression bomb");
        assert!(photo_acceptable(8000, 6000));
    }

    #[test]
    fn model_numbers_compare_loosely() {
        assert_eq!(normalise_model("6204-2RS"), normalise_model(" 6204 2rs "));
        assert_ne!(normalise_model("6204"), normalise_model("6205"));
    }
}
