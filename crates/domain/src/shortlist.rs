//! Which suppliers to ask for a quotation: about ten per request, ranked from the factory's
//! own record (who quoted these items before, who was cheapest, who replies, whom the factory
//! bought from or approved) and from the directory's categories. Every point a supplier
//! gets comes with its reason, so the staff member who checks the request before it is sent
//! sees why each supplier is on the list. The ranking only pre-selects; a person sends.

use crate::history::half_width;

/// How many suppliers are pre-selected when the caller does not say (CEO: "about 10").
pub const DEFAULT_PICKS: usize = 10;
/// Never more than an RFQ may carry (`MAX_RECIPIENTS` in the server).
pub const MAX_PICKS: usize = 20;

/// What the factory has decided about a directory supplier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    None,
    Proposed,
    Approved,
}

/// One verified directory supplier the factory has not rejected, with its record at this
/// factory. Counts are about this factory only; other customers' data is never used.
#[derive(Debug, Clone)]
pub struct Candidate<I> {
    pub id: I,
    pub name: String,
    pub categories: Vec<String>,
    /// Items of this request the supplier quoted for before.
    pub quoted_items: u32,
    /// Of those, items where its quotation was the lowest unit price received.
    pub cheapest_items: u32,
    /// Purchase orders the factory placed with it through us.
    pub orders: u32,
    /// The factory's own purchase records name it.
    pub bought_before: bool,
    pub approval: Approval,
    /// The factory's earlier RFQs sent to it, and how many it answered.
    pub invited: u32,
    pub responded: u32,
    /// Already a recipient of an open RFQ for this request.
    pub already_invited: bool,
}

/// Why a supplier ranks where it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    QuotedBefore { items: u32 },
    CheapestBefore { items: u32 },
    OrderedBefore { orders: u32 },
    BoughtBefore,
    Category { category: String },
    ApprovedByFactory,
    Replies { responded: u32, invited: u32 },
    NoReplies { invited: u32 },
    AlreadyInvited,
}

#[derive(Debug, Clone)]
pub struct Pick<I> {
    pub id: I,
    pub name: String,
    pub score: i64,
    /// Pre-selected for the RFQ.
    pub selected: bool,
    /// Has at least one sign it sells these items (not just a good record).
    pub relevant: bool,
    pub reasons: Vec<Reason>,
}

/// Words in a request line that mean a directory category of the first scope (consumables,
/// tools, electrical materials), in English and Japanese. A category also matches when its
/// own name appears in the line.
const CATEGORY_WORDS: &[(&str, &[&str])] = &[
    (
        "electrical",
        &[
            "cable",
            "wire",
            "breaker",
            "relay",
            "terminal",
            "switch",
            "fuse",
            "connector",
            "contactor",
            "plug",
            "socket",
            "led",
            "lamp",
            "sensor",
            "ケーブル",
            "電線",
            "ブレーカ",
            "リレー",
            "端子",
            "スイッチ",
            "ヒューズ",
            "コネクタ",
            "コンタクタ",
            "電磁接触器",
            "プラグ",
            "コンセント",
            "ランプ",
            "センサ",
            "配線",
            "圧着",
        ],
    ),
    (
        "tools",
        &[
            "drill",
            "end mill",
            "endmill",
            "tap",
            "wrench",
            "spanner",
            "driver",
            "pliers",
            "cutter",
            "insert",
            "bit",
            "saw",
            "hammer",
            "gauge",
            "ドリル",
            "エンドミル",
            "タップ",
            "レンチ",
            "スパナ",
            "ドライバ",
            "ペンチ",
            "ニッパ",
            "カッター",
            "チップ",
            "ビット",
            "ノコ",
            "ハンマ",
            "ゲージ",
            "工具",
            "刃物",
        ],
    ),
    (
        "consumables",
        &[
            "glove",
            "tape",
            "oil",
            "grease",
            "lubricant",
            "rag",
            "wipe",
            "abrasive",
            "sandpaper",
            "grinding",
            "disc",
            "filter",
            "mask",
            "cleaner",
            "solvent",
            "adhesive",
            "手袋",
            "軍手",
            "テープ",
            "オイル",
            "油",
            "グリス",
            "ウエス",
            "研磨",
            "砥石",
            "ペーパー",
            "ディスク",
            "フィルタ",
            "マスク",
            "洗浄",
            "溶剤",
            "接着",
        ],
    ),
    ("bearings", &["bearing", "ベアリング", "軸受"]),
    (
        "fasteners",
        &[
            "bolt",
            "nut",
            "screw",
            "washer",
            "ボルト",
            "ナット",
            "ねじ",
            "ネジ",
            "ワッシャ",
        ],
    ),
];

/// Text compared without width, case or repeated spaces.
fn fold(s: &str) -> String {
    half_width(s)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a Latin word occurs on its own ("tap" in "m6 tap", not in "tape"); other
/// scripts have no spaces, so any occurrence counts. Only short words need the check.
fn mentions(text: &str, word: &str) -> bool {
    if word.chars().count() > 4 || !word.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ') {
        return text.contains(word);
    }
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// The supplier's categories that a request line mentions, by the category's own name or
/// by a word of a known category.
pub fn matching_categories(categories: &[String], lines: &[String]) -> Vec<String> {
    let lines: Vec<String> = lines.iter().map(|l| fold(l)).collect();
    let mut out: Vec<String> = Vec::new();
    for category in categories {
        let c = fold(category);
        if c.chars().count() < 2 || out.iter().any(|o| fold(o) == c) {
            continue;
        }
        let words = CATEGORY_WORDS
            .iter()
            .find(|(name, _)| *name == c)
            .map_or(&[][..], |(_, w)| *w);
        if lines
            .iter()
            .any(|l| mentions(l, &c) || words.iter().any(|w| mentions(l, w)))
        {
            out.push(category.clone());
        }
    }
    out
}

/// Points and reasons for one supplier. `lines` is the request's text (description,
/// manufacturer, part number and notes of each line); `items` the number of lines.
pub fn score<I: Clone>(c: &Candidate<I>, lines: &[String], items: u32) -> Pick<I> {
    let mut reasons = Vec::new();
    let mut score: i64 = 0;
    let quoted = c.quoted_items.min(items);
    if quoted > 0 {
        score += 30 * i64::from(quoted);
        reasons.push(Reason::QuotedBefore { items: quoted });
    }
    let cheapest = c.cheapest_items.min(quoted);
    if cheapest > 0 {
        score += 20 * i64::from(cheapest);
        reasons.push(Reason::CheapestBefore { items: cheapest });
    }
    if c.orders > 0 {
        score += 6 * i64::from(c.orders.min(5));
        reasons.push(Reason::OrderedBefore { orders: c.orders });
    }
    if c.bought_before {
        score += 15;
        reasons.push(Reason::BoughtBefore);
    }
    let categories = matching_categories(&c.categories, lines);
    for category in categories.iter().take(3) {
        score += 12;
        reasons.push(Reason::Category {
            category: category.clone(),
        });
    }
    let relevant = quoted > 0 || c.orders > 0 || c.bought_before || !categories.is_empty();
    if c.approval == Approval::Approved {
        score += 10;
        reasons.push(Reason::ApprovedByFactory);
    }
    let responded = c.responded.min(c.invited);
    if c.invited >= 3 && responded == 0 {
        score -= 15;
        reasons.push(Reason::NoReplies { invited: c.invited });
    } else if c.invited >= 2 {
        score += i64::from(responded) * 20 / i64::from(c.invited);
        reasons.push(Reason::Replies {
            responded,
            invited: c.invited,
        });
    }
    if c.already_invited {
        reasons.push(Reason::AlreadyInvited);
    }
    Pick {
        id: c.id.clone(),
        name: c.name.clone(),
        score,
        selected: false,
        relevant,
        reasons,
    }
}

/// All candidates, best first (score, then name), with up to `picks` relevant suppliers not
/// already asked for this request pre-selected. The result is the same for the same input.
pub fn rank<I: Clone>(
    candidates: &[Candidate<I>],
    lines: &[String],
    items: u32,
    picks: usize,
) -> Vec<Pick<I>> {
    let mut out: Vec<(bool, Pick<I>)> = candidates
        .iter()
        .map(|c| (c.already_invited, score(c, lines, items)))
        .collect();
    out.sort_by(|(_, a), (_, b)| {
        b.score
            .cmp(&a.score)
            .then_with(|| fold(&a.name).cmp(&fold(&b.name)))
    });
    let mut left = picks.min(MAX_PICKS);
    for (invited, p) in &mut out {
        if left > 0 && !*invited && p.relevant && p.score > 0 {
            p.selected = true;
            left -= 1;
        }
    }
    out.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: u32, name: &str, categories: &[&str]) -> Candidate<u32> {
        Candidate {
            id,
            name: name.into(),
            categories: categories.iter().map(|c| (*c).to_string()).collect(),
            quoted_items: 0,
            cheapest_items: 0,
            orders: 0,
            bought_before: false,
            approval: Approval::None,
            invited: 0,
            responded: 0,
            already_invited: false,
        }
    }

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn categories_match_by_name_or_known_words_in_both_languages() {
        let cats = vec![
            "electrical".to_string(),
            "tools".to_string(),
            "bearings".to_string(),
        ];
        assert_eq!(
            matching_categories(&cats, &lines(&["ＶＶＦケーブル 2.0mm 100m"])),
            vec!["electrical".to_string()]
        );
        assert_eq!(
            matching_categories(
                &cats,
                &lines(&["Deep groove BEARING 6204ZZ", "超硬エンドミル φ6"])
            ),
            vec!["tools".to_string(), "bearings".to_string()]
        );
        // Short Latin words only on their own: "tape" is not a "tap".
        assert!(matching_categories(&["tools".to_string()], &lines(&["masking tape"])).is_empty());
        assert_eq!(
            matching_categories(&["tools".to_string()], &lines(&["M6 tap HSS"])),
            vec!["tools".to_string()]
        );
        // A brand or other category named by the directory matches its own name.
        assert_eq!(
            matching_categories(&["Misumi".to_string()], &lines(&["MISUMI shaft"])),
            vec!["Misumi".to_string()]
        );
    }

    #[test]
    fn the_factorys_record_ranks_above_a_category_alone() {
        let text = lines(&["ベアリング 6204ZZ"]);
        let mut quoted = cand(1, "B Trading", &[]);
        quoted.quoted_items = 1;
        quoted.cheapest_items = 1;
        let category = cand(2, "A Bearing", &["bearings"]);
        let unrelated = cand(3, "C Electric", &["electrical"]);
        let picks = rank(&[unrelated, category, quoted], &text, 1, 10);
        assert_eq!(
            picks.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(picks[0].score, 50);
        assert_eq!(
            picks[0].reasons,
            vec![
                Reason::QuotedBefore { items: 1 },
                Reason::CheapestBefore { items: 1 }
            ]
        );
        assert!(picks[0].selected && picks[1].selected);
        // Nothing says the electrical supplier sells bearings: listed, not pre-selected.
        assert!(!picks[2].selected && !picks[2].relevant);
    }

    #[test]
    fn suppliers_that_never_reply_drop_and_already_asked_are_not_selected_again() {
        let text = lines(&["glove nitrile L"]);
        let mut silent = cand(1, "Silent", &["consumables"]);
        silent.invited = 4;
        let mut replies = cand(2, "Replies", &["consumables"]);
        replies.invited = 4;
        replies.responded = 3;
        let mut asked = cand(3, "Asked", &["consumables"]);
        asked.already_invited = true;
        let picks = rank(&[silent, replies, asked], &text, 1, 10);
        assert_eq!(
            picks.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert_eq!(picks[0].score, 12 + 15);
        assert!(picks[0].selected);
        assert!(!picks[1].selected);
        assert!(picks[1].reasons.contains(&Reason::AlreadyInvited));
        // 12 for the category, −15 for four requests without a reply.
        assert_eq!(picks[2].score, -3);
        assert!(!picks[2].selected);
    }

    #[test]
    fn about_ten_are_selected_and_order_is_stable() {
        let text = lines(&["relay 24V"]);
        let all: Vec<Candidate<u32>> = (0..15)
            .map(|i| cand(i, &format!("Supplier {:02}", 14 - i), &["electrical"]))
            .collect();
        let picks = rank(&all, &text, 1, DEFAULT_PICKS);
        assert_eq!(picks.iter().filter(|p| p.selected).count(), 10);
        // Equal scores: by name.
        assert_eq!(picks[0].name, "Supplier 00");
        assert!(picks[..10].iter().all(|p| p.selected));
        assert_eq!(
            rank(&all, &text, 1, 99)
                .iter()
                .filter(|p| p.selected)
                .count(),
            15
        );
        let again = rank(&all, &text, 1, DEFAULT_PICKS);
        assert_eq!(
            picks.iter().map(|p| p.id).collect::<Vec<_>>(),
            again.iter().map(|p| p.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn counts_are_bounded() {
        let mut c = cand(1, "X", &[]);
        c.quoted_items = 9;
        c.cheapest_items = 9;
        c.orders = 40;
        c.responded = 7;
        c.invited = 2;
        let p = score(&c, &[], 2);
        // 2 items quoted, 2 cheapest, orders capped at 5, replies capped at invitations.
        assert_eq!(p.score, 60 + 40 + 30 + 20);
        assert!(p.reasons.contains(&Reason::Replies {
            responded: 2,
            invited: 2
        }));
    }
}
