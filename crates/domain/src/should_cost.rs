//! Should-cost: what one unit of an item ought to cost, built up from its inputs, and how a
//! quoted price compares with it, with the factory's own lowest price and with the other
//! quotations (docs/agents/DESIGN.md, L5).
//!
//! Cost build-up per unit, in thousandths of a minor currency unit ("milli"):
//! - material: Σ quantity × unit price per kg / unit, plus scrap;
//! - processes: Σ machine time × hourly rate, plus set-up time × rate amortised over the lot;
//! - overhead: basis points on material + processes;
//! - logistics: per-lot cost amortised over the lot;
//! - margin: basis points on everything above.
//!
//! Integer arithmetic only (i128 inside, rounded half up at each division); the same inputs
//! always give the same result. Everything here is a reference for a negotiation, never a
//! decision: nothing in this module changes compliance, ranking or any approval.

use std::fmt;

/// Basis points in one whole.
pub const BPS: i64 = 10_000;
/// A quote this far (or further) above the should-cost or the lowest price paid is worth a
/// negotiation, unless the organisation set its own alert line.
pub const DEFAULT_GAP_THRESHOLD_BPS: i64 = 1_000;
/// The sensitivity step: each input is moved by this much (10 %).
pub const SENSITIVITY_STEP_BPS: i64 = 1_000;

pub const MAX_LINES: usize = 50;
pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_LOT_SIZE: u32 = 10_000_000;
/// Quantities in thousandths (of a kg or unit), prices in minor units, per input.
pub const MAX_QUANTITY_MILLI: i64 = 1_000_000_000_000;
pub const MAX_PRICE_MINOR: i64 = 1_000_000_000_000;
/// Machine time per unit in milliseconds (at most 30 days).
pub const MAX_CYCLE_MS: i64 = 30 * 24 * 3_600_000;
pub const MAX_SETUP_MINUTES: u32 = 100_000;
/// Overhead and margin are at most 1000 %; scrap at most 100 %.
pub const MAX_RATE_BPS: u32 = 100_000;
pub const MAX_SCRAP_BPS: u32 = 10_000;

/// One material per unit: `quantity_milli` thousandths of a kg (or of a unit) at
/// `unit_price_minor` per kg (or unit), plus `scrap_bps` of it lost in production.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub name: String,
    pub quantity_milli: i64,
    pub unit_price_minor: i64,
    pub scrap_bps: u32,
}

/// One process step: `cycle_ms` of machine time per unit at `rate_minor_per_hour`, plus
/// `setup_minutes` once per lot at the same rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub name: String,
    pub cycle_ms: i64,
    pub rate_minor_per_hour: i64,
    pub setup_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostInputs {
    /// Units per production lot (set-up and logistics are spread over it).
    pub lot_size: u32,
    pub materials: Vec<Material>,
    pub processes: Vec<Process>,
    pub overhead_bps: u32,
    pub logistics_minor_per_lot: i64,
    pub margin_bps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostError {
    LotSize,
    TooManyLines,
    Name,
    Quantity,
    Price,
    Cycle,
    Setup,
    Rate,
    Scrap,
    Logistics,
    /// Nothing to build a cost from (no material and no process).
    Empty,
    Overflow,
}

impl CostError {
    pub const fn code(self) -> &'static str {
        match self {
            CostError::LotSize => "lot_size",
            CostError::TooManyLines => "too_many_lines",
            CostError::Name => "name",
            CostError::Quantity => "quantity_milli",
            CostError::Price => "unit_price_minor",
            CostError::Cycle => "cycle_ms",
            CostError::Setup => "setup_minutes",
            CostError::Rate => "rate_bps",
            CostError::Scrap => "scrap_bps",
            CostError::Logistics => "logistics_minor_per_lot",
            CostError::Empty => "empty_model",
            CostError::Overflow => "overflow",
        }
    }
}

impl fmt::Display for CostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for CostError {}

fn name_ok(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n.chars().count() <= MAX_NAME_CHARS && !n.chars().any(char::is_control)
}

/// Checks every bound; the cost of valid inputs never overflows.
pub fn validate(inputs: &CostInputs) -> Result<(), CostError> {
    if inputs.lot_size == 0 || inputs.lot_size > MAX_LOT_SIZE {
        return Err(CostError::LotSize);
    }
    if inputs.materials.len() > MAX_LINES || inputs.processes.len() > MAX_LINES {
        return Err(CostError::TooManyLines);
    }
    if inputs.materials.is_empty() && inputs.processes.is_empty() {
        return Err(CostError::Empty);
    }
    for m in &inputs.materials {
        if !name_ok(&m.name) {
            return Err(CostError::Name);
        }
        if !(0..=MAX_QUANTITY_MILLI).contains(&m.quantity_milli) {
            return Err(CostError::Quantity);
        }
        if !(0..=MAX_PRICE_MINOR).contains(&m.unit_price_minor) {
            return Err(CostError::Price);
        }
        if m.scrap_bps > MAX_SCRAP_BPS {
            return Err(CostError::Scrap);
        }
    }
    for p in &inputs.processes {
        if !name_ok(&p.name) {
            return Err(CostError::Name);
        }
        if !(0..=MAX_CYCLE_MS).contains(&p.cycle_ms) {
            return Err(CostError::Cycle);
        }
        if !(0..=MAX_PRICE_MINOR).contains(&p.rate_minor_per_hour) {
            return Err(CostError::Price);
        }
        if p.setup_minutes > MAX_SETUP_MINUTES {
            return Err(CostError::Setup);
        }
    }
    if inputs.overhead_bps > MAX_RATE_BPS || inputs.margin_bps > MAX_RATE_BPS {
        return Err(CostError::Rate);
    }
    if !(0..=MAX_PRICE_MINOR).contains(&inputs.logistics_minor_per_lot) {
        return Err(CostError::Logistics);
    }
    Ok(())
}

/// `n / d` rounded half up (`n >= 0`, `d > 0`).
fn div_round(n: i128, d: i128) -> i128 {
    (n * 2 + d) / (d * 2)
}

/// One line of the build-up (per unit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostLine {
    /// `material` | `process`
    pub kind: &'static str,
    pub name: String,
    pub milli: i64,
}

/// What one unit should cost, by component (thousandths of a minor unit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakdown {
    pub material_milli: i64,
    pub process_milli: i64,
    pub overhead_milli: i64,
    pub logistics_milli: i64,
    pub margin_milli: i64,
    pub total_milli: i64,
    /// `total_milli` rounded half up to the minor unit.
    pub unit_cost_minor: i64,
    pub lines: Vec<CostLine>,
}

fn material_milli(m: &Material) -> i128 {
    // quantity (thousandths) × price per whole unit = thousandths of a minor unit.
    let base = i128::from(m.quantity_milli) * i128::from(m.unit_price_minor);
    div_round(
        base * i128::from(BPS + i64::from(m.scrap_bps)),
        i128::from(BPS),
    )
}

fn process_milli(p: &Process, lot_size: u32) -> i128 {
    let rate = i128::from(p.rate_minor_per_hour);
    // ms × (rate / 3,600,000 per ms) × 1000 milli = ms × rate / 3600.
    let run = div_round(i128::from(p.cycle_ms) * rate, 3_600);
    // minutes × rate / 60 per lot, in milli, over the lot.
    let setup = div_round(
        i128::from(p.setup_minutes) * rate * 1_000,
        60 * i128::from(lot_size),
    );
    run + setup
}

fn to_i64(v: i128) -> Result<i64, CostError> {
    i64::try_from(v).map_err(|_| CostError::Overflow)
}

/// The cost build-up of one unit.
pub fn build_up(inputs: &CostInputs) -> Result<Breakdown, CostError> {
    validate(inputs)?;
    let mut lines = Vec::with_capacity(inputs.materials.len() + inputs.processes.len());
    let mut material: i128 = 0;
    for m in &inputs.materials {
        let v = material_milli(m);
        material += v;
        lines.push(CostLine {
            kind: "material",
            name: m.name.trim().to_string(),
            milli: to_i64(v)?,
        });
    }
    let mut process: i128 = 0;
    for p in &inputs.processes {
        let v = process_milli(p, inputs.lot_size);
        process += v;
        lines.push(CostLine {
            kind: "process",
            name: p.name.trim().to_string(),
            milli: to_i64(v)?,
        });
    }
    let overhead = div_round(
        (material + process) * i128::from(inputs.overhead_bps),
        i128::from(BPS),
    );
    let logistics = div_round(
        i128::from(inputs.logistics_minor_per_lot) * 1_000,
        i128::from(inputs.lot_size),
    );
    let before_margin = material + process + overhead + logistics;
    let margin = div_round(
        before_margin * i128::from(inputs.margin_bps),
        i128::from(BPS),
    );
    let total = before_margin + margin;
    Ok(Breakdown {
        material_milli: to_i64(material)?,
        process_milli: to_i64(process)?,
        overhead_milli: to_i64(overhead)?,
        logistics_milli: to_i64(logistics)?,
        margin_milli: to_i64(margin)?,
        total_milli: to_i64(total)?,
        unit_cost_minor: to_i64(div_round(total, 1_000))?,
        lines,
    })
}

/// One input moved by [`SENSITIVITY_STEP_BPS`] and what it did to the unit cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Driver {
    /// e.g. `material.0.unit_price`, `process.1.cycle`, `overhead`, `logistics`, `margin`.
    pub input: String,
    /// The line's name for material and process inputs, else empty.
    pub name: String,
    /// Increase of the unit cost when this input rises by 10 % (thousandths of a minor unit).
    pub delta_milli: i64,
    /// The same as a share of the unit cost (basis points).
    pub delta_bps: i64,
}

fn bump(v: i64) -> i64 {
    // +10 %, rounded half up; small values still move (1 → 1, so they rank last).
    i64::try_from(div_round(
        i128::from(v) * i128::from(BPS + SENSITIVITY_STEP_BPS),
        i128::from(BPS),
    ))
    .unwrap_or(i64::MAX)
}

fn bump_u32(v: u32, max: u32) -> u32 {
    u32::try_from(bump(i64::from(v)).min(i64::from(max))).unwrap_or(max)
}

/// Which inputs drive the unit cost most: each input raised by 10 % in turn (bounds still
/// apply, so an input at its maximum does not move), largest effect first; ties keep the
/// input order. Inputs that change nothing are left out.
pub fn sensitivity(inputs: &CostInputs) -> Result<Vec<Driver>, CostError> {
    let base = build_up(inputs)?;
    let mut variants: Vec<(String, String, CostInputs)> = Vec::new();
    for (i, m) in inputs.materials.iter().enumerate() {
        let mut q = inputs.clone();
        q.materials[i].quantity_milli = bump(m.quantity_milli).min(MAX_QUANTITY_MILLI);
        variants.push((format!("material.{i}.quantity"), m.name.clone(), q));
        let mut p = inputs.clone();
        p.materials[i].unit_price_minor = bump(m.unit_price_minor).min(MAX_PRICE_MINOR);
        variants.push((format!("material.{i}.unit_price"), m.name.clone(), p));
    }
    for (i, pr) in inputs.processes.iter().enumerate() {
        let mut c = inputs.clone();
        c.processes[i].cycle_ms = bump(pr.cycle_ms).min(MAX_CYCLE_MS);
        variants.push((format!("process.{i}.cycle"), pr.name.clone(), c));
        let mut r = inputs.clone();
        r.processes[i].rate_minor_per_hour = bump(pr.rate_minor_per_hour).min(MAX_PRICE_MINOR);
        variants.push((format!("process.{i}.rate"), pr.name.clone(), r));
        let mut s = inputs.clone();
        s.processes[i].setup_minutes = bump_u32(pr.setup_minutes, MAX_SETUP_MINUTES);
        variants.push((format!("process.{i}.setup"), pr.name.clone(), s));
    }
    let mut o = inputs.clone();
    o.overhead_bps = bump_u32(inputs.overhead_bps, MAX_RATE_BPS);
    variants.push(("overhead".into(), String::new(), o));
    let mut l = inputs.clone();
    l.logistics_minor_per_lot = bump(inputs.logistics_minor_per_lot).min(MAX_PRICE_MINOR);
    variants.push(("logistics".into(), String::new(), l));
    let mut g = inputs.clone();
    g.margin_bps = bump_u32(inputs.margin_bps, MAX_RATE_BPS);
    variants.push(("margin".into(), String::new(), g));

    let mut out = Vec::with_capacity(variants.len());
    for (input, name, v) in variants {
        let delta = build_up(&v)?.total_milli - base.total_milli;
        if delta <= 0 {
            continue;
        }
        let delta_bps = if base.total_milli > 0 {
            to_i64(div_round(
                i128::from(delta) * i128::from(BPS),
                i128::from(base.total_milli),
            ))?
        } else {
            0
        };
        out.push(Driver {
            input,
            name: name.trim().to_string(),
            delta_milli: delta,
            delta_bps,
        });
    }
    // Stable: equal effects keep the input order.
    out.sort_by_key(|d| std::cmp::Reverse(d.delta_milli));
    Ok(out)
}

/// How far a quoted unit price (minor units) is above (+) or below (−) a reference price in
/// thousandths of a minor unit, in basis points, rounded half away from zero. `None` when
/// the reference is not positive.
pub fn gap_bps(reference_milli: i64, quoted_minor: i64) -> Option<i64> {
    if reference_milli <= 0 {
        return None;
    }
    let diff = (i128::from(quoted_minor) * 1_000 - i128::from(reference_milli)) * i128::from(BPS);
    let den = i128::from(reference_milli);
    let q = if diff >= 0 {
        div_round(diff, den)
    } else {
        -div_round(-diff, den)
    };
    i64::try_from(q).ok()
}

/// A gap is worth a negotiation only when it is strictly above the threshold.
pub fn above_threshold(gap_bps: Option<i64>, threshold_bps: i64) -> bool {
    matches!(gap_bps, Some(g) if g > threshold_bps)
}

/// A past price of the item (same currency as the quotation), oldest first or in any order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PastPrice {
    /// Days since 1970-01-01 (`history::Day::epoch_days`).
    pub day: i64,
    pub unit_price_milli: i64,
}

/// The price trend over the factory's own purchases: the latest price against the earliest,
/// in basis points. `None` with fewer than two purchases on different days.
pub fn trend_bps(points: &[PastPrice]) -> Option<i64> {
    // Ties on the day keep the first one seen (deterministic for the same input).
    let mut first: Option<&PastPrice> = None;
    let mut last: Option<&PastPrice> = None;
    for p in points {
        if first.is_none_or(|f| p.day < f.day) {
            first = Some(p);
        }
        if last.is_none_or(|l| p.day > l.day) {
            last = Some(p);
        }
    }
    let (first, last) = (first?, last?);
    if first.day == last.day || first.unit_price_milli <= 0 {
        return None;
    }
    let diff =
        (i128::from(last.unit_price_milli) - i128::from(first.unit_price_milli)) * i128::from(BPS);
    let den = i128::from(first.unit_price_milli);
    let q = if diff >= 0 {
        div_round(diff, den)
    } else {
        -div_round(-diff, den)
    };
    i64::try_from(q).ok()
}

/// What is known about one quoted line (all prices in the quotation's currency).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInput {
    pub quantity: u32,
    pub quoted_unit_minor: i64,
    /// The factory's own past prices of the item.
    pub history: Vec<PastPrice>,
    /// Unit should-cost from the item's cost model.
    pub should_cost_milli: Option<i64>,
    /// Unit prices of every current quotation for this line, this one included.
    pub competing_minor: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub should_cost_bps: i64,
    pub history_bps: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            should_cost_bps: DEFAULT_GAP_THRESHOLD_BPS,
            history_bps: DEFAULT_GAP_THRESHOLD_BPS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Signal {
    AboveShouldCost,
    AboveHistoricalLow,
}

impl Signal {
    pub const fn code(self) -> &'static str {
        match self {
            Signal::AboveShouldCost => "above_should_cost",
            Signal::AboveHistoricalLow => "above_historical_low",
        }
    }
}

/// The deterministic facts of one quoted line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineFacts {
    pub purchases: usize,
    /// Lowest and latest price paid (thousandths of a minor unit).
    pub historical_low_milli: Option<i64>,
    pub last_paid_milli: Option<i64>,
    pub trend_bps: Option<i64>,
    pub history_gap_bps: Option<i64>,
    pub should_cost_milli: Option<i64>,
    pub should_cost_gap_bps: Option<i64>,
    pub competing_quotes: usize,
    pub competing_low_minor: Option<i64>,
    pub competing_high_minor: Option<i64>,
    /// (highest − lowest) / lowest of the current quotes, basis points.
    pub spread_bps: Option<i64>,
    /// The lowest of should-cost, lowest price paid and lowest current quote.
    pub best_reference_milli: Option<i64>,
    /// (quoted − best reference) × quantity, never negative, rounded down to the minor unit.
    pub savings_opportunity_minor: i64,
    pub signals: Vec<Signal>,
}

/// The facts of one quoted line against its references.
pub fn assess_line(line: &LineInput, t: Thresholds) -> LineFacts {
    let latest = line.history.iter().max_by_key(|p| p.day);
    let low = line.history.iter().map(|p| p.unit_price_milli).min();
    let history_gap = low.and_then(|l| gap_bps(l, line.quoted_unit_minor));
    let should_gap = line
        .should_cost_milli
        .and_then(|s| gap_bps(s, line.quoted_unit_minor));
    let c_low = line.competing_minor.iter().copied().min();
    let c_high = line.competing_minor.iter().copied().max();
    let spread = match (c_low, c_high) {
        (Some(l), Some(h)) if line.competing_minor.len() > 1 => gap_bps(l * 1_000, h),
        _ => None,
    };
    let best = [
        line.should_cost_milli,
        low,
        c_low.map(|c| c.saturating_mul(1_000)),
    ]
    .into_iter()
    .flatten()
    .min();
    let savings = best.map_or(0, |b| {
        let over = i128::from(line.quoted_unit_minor) * 1_000 - i128::from(b);
        let total = over.max(0) * i128::from(line.quantity) / 1_000;
        i64::try_from(total).unwrap_or(i64::MAX)
    });
    let mut signals = Vec::new();
    if above_threshold(should_gap, t.should_cost_bps) {
        signals.push(Signal::AboveShouldCost);
    }
    if above_threshold(history_gap, t.history_bps) {
        signals.push(Signal::AboveHistoricalLow);
    }
    LineFacts {
        purchases: line.history.len(),
        historical_low_milli: low,
        last_paid_milli: latest.map(|p| p.unit_price_milli),
        trend_bps: trend_bps(&line.history),
        history_gap_bps: history_gap,
        should_cost_milli: line.should_cost_milli,
        should_cost_gap_bps: should_gap,
        competing_quotes: line.competing_minor.len(),
        competing_low_minor: c_low,
        competing_high_minor: c_high,
        spread_bps: spread,
        best_reference_milli: best,
        savings_opportunity_minor: savings,
        signals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steel() -> Material {
        Material {
            name: "SUS304".into(),
            quantity_milli: 250, // 0.25 kg
            unit_price_minor: 800,
            scrap_bps: 1_000,
        }
    }

    fn lathe() -> Process {
        Process {
            name: "旋盤".into(),
            cycle_ms: 90_000, // 90 s
            rate_minor_per_hour: 6_000,
            setup_minutes: 30,
        }
    }

    fn part() -> CostInputs {
        CostInputs {
            lot_size: 100,
            materials: vec![steel()],
            processes: vec![lathe()],
            overhead_bps: 1_500,
            logistics_minor_per_lot: 5_000,
            margin_bps: 1_000,
        }
    }

    #[test]
    fn worked_example() {
        let b = build_up(&part()).unwrap();
        // 0.25 kg × 800 = 200 yen, +10 % scrap = 220 yen.
        assert_eq!(b.material_milli, 220_000);
        // 90 s at 6000/h = 150 yen; set-up 30 min at 6000/h = 3000 yen over 100 = 30 yen.
        assert_eq!(b.process_milli, 180_000);
        // 15 % of 400 yen.
        assert_eq!(b.overhead_milli, 60_000);
        // 5000 yen per lot of 100.
        assert_eq!(b.logistics_milli, 50_000);
        // 10 % of 510 yen.
        assert_eq!(b.margin_milli, 51_000);
        assert_eq!(b.total_milli, 561_000);
        assert_eq!(b.unit_cost_minor, 561);
        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.lines[0].kind, "material");
        assert_eq!(b.lines[1].milli, 180_000);
    }

    #[test]
    fn rounding_is_half_up_and_exact() {
        let inputs = CostInputs {
            lot_size: 3,
            materials: vec![Material {
                name: "wire".into(),
                quantity_milli: 1,
                unit_price_minor: 1,
                scrap_bps: 0,
            }],
            processes: vec![Process {
                name: "cut".into(),
                cycle_ms: 1,
                rate_minor_per_hour: 1_800, // 0.5 milli per ms → rounds to 1
                setup_minutes: 1,           // 1800/60 = 30 yen per lot / 3 = 10 yen
            }],
            overhead_bps: 0,
            logistics_minor_per_lot: 1, // 1000/3 = 333.33 → 333
            margin_bps: 0,
        };
        let b = build_up(&inputs).unwrap();
        assert_eq!(b.material_milli, 1);
        assert_eq!(b.process_milli, 1 + 10_000);
        assert_eq!(b.logistics_milli, 333);
        assert_eq!(b.total_milli, 10_335);
        assert_eq!(b.unit_cost_minor, 10);
    }

    #[test]
    fn bounds_are_enforced() {
        type Change = Box<dyn Fn(&mut CostInputs)>;
        let cases: Vec<(Change, CostError)> = vec![
            (Box::new(|i| i.lot_size = 0), CostError::LotSize),
            (
                Box::new(|i| i.lot_size = MAX_LOT_SIZE + 1),
                CostError::LotSize,
            ),
            (
                Box::new(|i| i.materials = vec![steel(); MAX_LINES + 1]),
                CostError::TooManyLines,
            ),
            (
                Box::new(|i| i.materials[0].name = " ".into()),
                CostError::Name,
            ),
            (
                Box::new(|i| i.processes[0].name = "a\u{0}b".into()),
                CostError::Name,
            ),
            (
                Box::new(|i| i.materials[0].name = "x".repeat(101)),
                CostError::Name,
            ),
            (
                Box::new(|i| i.materials[0].quantity_milli = -1),
                CostError::Quantity,
            ),
            (
                Box::new(|i| i.materials[0].unit_price_minor = MAX_PRICE_MINOR + 1),
                CostError::Price,
            ),
            (
                Box::new(|i| i.materials[0].scrap_bps = 10_001),
                CostError::Scrap,
            ),
            (Box::new(|i| i.processes[0].cycle_ms = -5), CostError::Cycle),
            (
                Box::new(|i| i.processes[0].rate_minor_per_hour = -1),
                CostError::Price,
            ),
            (
                Box::new(|i| i.processes[0].setup_minutes = MAX_SETUP_MINUTES + 1),
                CostError::Setup,
            ),
            (
                Box::new(|i| i.overhead_bps = MAX_RATE_BPS + 1),
                CostError::Rate,
            ),
            (
                Box::new(|i| i.margin_bps = MAX_RATE_BPS + 1),
                CostError::Rate,
            ),
            (
                Box::new(|i| i.logistics_minor_per_lot = -1),
                CostError::Logistics,
            ),
            (
                Box::new(|i| {
                    i.materials.clear();
                    i.processes.clear();
                }),
                CostError::Empty,
            ),
        ];
        for (n, (change, want)) in cases.into_iter().enumerate() {
            let mut i = part();
            change(&mut i);
            assert_eq!(build_up(&i), Err(want), "case {n}");
            assert_eq!(sensitivity(&i), Err(want), "case {n}");
        }
    }

    #[test]
    fn largest_valid_inputs_do_not_overflow() {
        let m = Material {
            name: "max".into(),
            quantity_milli: MAX_QUANTITY_MILLI,
            unit_price_minor: MAX_PRICE_MINOR,
            scrap_bps: MAX_SCRAP_BPS,
        };
        let inputs = CostInputs {
            lot_size: 1,
            materials: vec![m; MAX_LINES],
            processes: vec![],
            overhead_bps: MAX_RATE_BPS,
            logistics_minor_per_lot: MAX_PRICE_MINOR,
            margin_bps: MAX_RATE_BPS,
        };
        // Far beyond i64 milli: refused as overflow, never wrapped or panicking.
        assert_eq!(build_up(&inputs), Err(CostError::Overflow));
    }

    #[test]
    fn sensitivity_ranks_the_biggest_driver_first() {
        let d = sensitivity(&part()).unwrap();
        let names: Vec<&str> = d.iter().map(|x| x.input.as_str()).collect();
        // Material quantity and price both move 22 yen + overhead + margin; tied, input order.
        assert_eq!(names[0], "material.0.quantity");
        assert_eq!(names[1], "material.0.unit_price");
        assert_eq!(d[0].delta_milli, d[1].delta_milli);
        // +10 % of 220 yen = 22 yen; ×1.15 overhead ×1.1 margin = 27.83 yen.
        assert_eq!(d[0].delta_milli, 27_830);
        assert_eq!(d[0].delta_bps, 496);
        assert!(d.windows(2).all(|w| w[0].delta_milli >= w[1].delta_milli));
        assert!(d.iter().all(|x| x.delta_milli > 0));
        // Rate moves run and set-up; cycle only the run part.
        let rate = d.iter().find(|x| x.input == "process.0.rate").unwrap();
        let cycle = d.iter().find(|x| x.input == "process.0.cycle").unwrap();
        assert!(rate.delta_milli > cycle.delta_milli);
        assert_eq!(rate.name, "旋盤");
    }

    #[test]
    fn inputs_at_zero_are_not_drivers() {
        let mut i = part();
        i.margin_bps = 0;
        i.logistics_minor_per_lot = 0;
        let d = sensitivity(&i).unwrap();
        assert!(
            d.iter()
                .all(|x| x.input != "margin" && x.input != "logistics")
        );
    }

    #[test]
    fn gap_and_threshold() {
        // Quote 1100 vs reference 1000.000.
        assert_eq!(gap_bps(1_000_000, 1_100), Some(1_000));
        assert_eq!(gap_bps(1_000_000, 900), Some(-1_000));
        assert_eq!(gap_bps(1_000_000, 1_000), Some(0));
        assert_eq!(gap_bps(0, 1_000), None);
        assert_eq!(gap_bps(-1, 1_000), None);
        // 1/3 → 3333.33 bps → 3333; 2/3 → 6666.67 → 6667 (half away from zero).
        assert_eq!(gap_bps(3_000, 4), Some(3_333));
        assert_eq!(gap_bps(3_000, 5), Some(6_667));
        assert_eq!(gap_bps(3_000, 2), Some(-3_333));
        // Strictly above: exactly at the line is not a finding.
        assert!(!above_threshold(Some(1_000), 1_000));
        assert!(above_threshold(Some(1_001), 1_000));
        assert!(!above_threshold(None, 0));
        assert!(!above_threshold(Some(-5_000), 1_000));
        // Huge numbers saturate to None, never panic.
        assert_eq!(gap_bps(1, i64::MAX), None);
    }

    #[test]
    fn trend_uses_earliest_and_latest() {
        let p = |day, price| PastPrice {
            day,
            unit_price_milli: price,
        };
        assert_eq!(trend_bps(&[]), None);
        assert_eq!(trend_bps(&[p(1, 1_000)]), None);
        assert_eq!(trend_bps(&[p(1, 1_000), p(1, 2_000)]), None);
        assert_eq!(
            trend_bps(&[p(9, 1_100), p(1, 1_000), p(5, 5_000)]),
            Some(1_000)
        );
        assert_eq!(trend_bps(&[p(1, 1_000), p(2, 800)]), Some(-2_000));
        assert_eq!(trend_bps(&[p(1, 0), p(2, 800)]), None);
    }

    fn line() -> LineInput {
        LineInput {
            quantity: 100,
            quoted_unit_minor: 1_300,
            history: vec![
                PastPrice {
                    day: 10,
                    unit_price_milli: 1_000_000,
                },
                PastPrice {
                    day: 20,
                    unit_price_milli: 1_100_000,
                },
            ],
            should_cost_milli: Some(1_100_000),
            competing_minor: vec![1_300, 1_200, 1_500],
        }
    }

    #[test]
    fn line_facts() {
        let f = assess_line(&line(), Thresholds::default());
        assert_eq!(f.purchases, 2);
        assert_eq!(f.historical_low_milli, Some(1_000_000));
        assert_eq!(f.last_paid_milli, Some(1_100_000));
        assert_eq!(f.trend_bps, Some(1_000));
        assert_eq!(f.history_gap_bps, Some(3_000));
        assert_eq!(f.should_cost_gap_bps, Some(1_818));
        assert_eq!(f.competing_quotes, 3);
        assert_eq!(f.competing_low_minor, Some(1_200));
        assert_eq!(f.competing_high_minor, Some(1_500));
        assert_eq!(f.spread_bps, Some(2_500));
        assert_eq!(f.best_reference_milli, Some(1_000_000));
        // (1300 − 1000) × 100.
        assert_eq!(f.savings_opportunity_minor, 30_000);
        assert_eq!(
            f.signals,
            vec![Signal::AboveShouldCost, Signal::AboveHistoricalLow]
        );
    }

    #[test]
    fn thresholds_decide_the_signals() {
        let table = [
            (1_818, 3_000, vec![]),
            (1_817, 3_000, vec![Signal::AboveShouldCost]),
            (2_000, 2_999, vec![Signal::AboveHistoricalLow]),
            (
                0,
                0,
                vec![Signal::AboveShouldCost, Signal::AboveHistoricalLow],
            ),
        ];
        for (sc, h, want) in table {
            let t = Thresholds {
                should_cost_bps: sc,
                history_bps: h,
            };
            assert_eq!(assess_line(&line(), t).signals, want, "{sc}/{h}");
        }
    }

    #[test]
    fn nothing_known_means_no_signal_and_no_saving() {
        let l = LineInput {
            quantity: 5,
            quoted_unit_minor: 999,
            history: vec![],
            should_cost_milli: None,
            competing_minor: vec![999],
        };
        let f = assess_line(&l, Thresholds::default());
        assert!(f.signals.is_empty());
        assert_eq!(f.spread_bps, None);
        assert_eq!(f.best_reference_milli, Some(999_000));
        assert_eq!(f.savings_opportunity_minor, 0);
        assert_eq!(f.trend_bps, None);
    }

    #[test]
    fn quote_below_every_reference_saves_nothing() {
        let mut l = line();
        l.quoted_unit_minor = 900;
        l.competing_minor = vec![900, 1_200];
        let f = assess_line(&l, Thresholds::default());
        assert!(f.signals.is_empty());
        assert_eq!(f.savings_opportunity_minor, 0);
        assert_eq!(f.history_gap_bps, Some(-1_000));
    }

    /// Deterministic pseudo-random inputs (no proptest in the workspace): invariants that
    /// must hold for every valid model.
    #[test]
    fn invariants_hold_for_many_random_models() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |max: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % (max + 1)
        };
        for _ in 0..500 {
            let mats = next(3) as usize;
            let procs = if mats == 0 { 1 + next(2) } else { next(3) } as usize;
            let inputs = CostInputs {
                lot_size: 1 + next(10_000) as u32,
                materials: (0..mats)
                    .map(|i| Material {
                        name: format!("m{i}"),
                        quantity_milli: next(5_000_000) as i64,
                        unit_price_minor: next(100_000) as i64,
                        scrap_bps: next(3_000) as u32,
                    })
                    .collect(),
                processes: (0..procs)
                    .map(|i| Process {
                        name: format!("p{i}"),
                        cycle_ms: next(3_600_000) as i64,
                        rate_minor_per_hour: next(50_000) as i64,
                        setup_minutes: next(600) as u32,
                    })
                    .collect(),
                overhead_bps: next(5_000) as u32,
                logistics_minor_per_lot: next(1_000_000) as i64,
                margin_bps: next(5_000) as u32,
            };
            let b = build_up(&inputs).unwrap();
            // Same input, same answer.
            assert_eq!(build_up(&inputs).unwrap(), b);
            // Components add up; nothing negative; rounding within half a minor unit.
            assert_eq!(
                b.total_milli,
                b.material_milli
                    + b.process_milli
                    + b.overhead_milli
                    + b.logistics_milli
                    + b.margin_milli
            );
            assert_eq!(
                b.lines.iter().map(|l| l.milli).sum::<i64>(),
                b.material_milli + b.process_milli
            );
            assert!(b.total_milli >= 0);
            assert!((b.unit_cost_minor * 1_000 - b.total_milli).abs() <= 500);
            // Raising any input never lowers the cost.
            let d = sensitivity(&inputs).unwrap();
            assert!(d.iter().all(|x| x.delta_milli > 0));
            assert!(d.windows(2).all(|w| w[0].delta_milli >= w[1].delta_milli));
            // A bigger lot never costs more per unit.
            let mut bigger = inputs.clone();
            bigger.lot_size = (inputs.lot_size * 2).min(MAX_LOT_SIZE);
            assert!(build_up(&bigger).unwrap().total_milli <= b.total_milli);
            // The quote at exactly the should-cost has no gap.
            if b.total_milli > 0 && b.total_milli % 1_000 == 0 {
                assert_eq!(gap_bps(b.total_milli, b.unit_cost_minor), Some(0));
            }
            if b.total_milli > 0 {
                // One minor unit above the rounded cost is never below it.
                let g = gap_bps(b.total_milli, b.unit_cost_minor + 1).unwrap();
                assert!(g >= 0);
            }
        }
    }
}
