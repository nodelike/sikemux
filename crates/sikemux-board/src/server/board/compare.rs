//! Reads a project's run records side by side, so an agent can see what each of Auto's switches
//! costs and buys, and read the runs where a reviewer's judgement did not hold up.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::record::RunRecord;
use super::{eta, recover, Card};

/// Fewer landed runs than this on either side of a switch say too little to go by.
const ENOUGH_LANDED: usize = 10;
const MAX_CASES: usize = 20;
const MAX_ASKED: usize = 5;
const MAX_ASKED_CHARS: usize = 300;

#[derive(Default)]
struct Side {
    runs: usize,
    landed: usize,
    broke: usize,
    rounds: u32,
    trips: u32,
    cents: Vec<u64>,
    minutes: Vec<u64>,
}

pub trait SideExt: Sized {
    fn add(&mut self, record: &RunRecord);

    fn view(self) -> Value;
}

impl SideExt for Side {
    fn add(&mut self, record: &RunRecord) {
        self.runs += 1;
        self.trips += record.progress_trips;
        let Some(finished_at) = record.finished_at else {
            return;
        };
        self.landed += 1;
        self.broke += usize::from(record.broke_after_landing);
        self.rounds += record.rounds;
        self.cents
            .extend(record.cost_usd.map(|cost| (cost * 100.0).round() as u64));
        self.minutes
            .push(finished_at.saturating_sub(record.started_at) / 60_000);
    }

    fn view(self) -> Value {
        let rounds_per_landed =
            (self.landed > 0).then(|| f64::from(self.rounds) / self.landed as f64);
        let runs_with_cost = self.cents.len();
        json!({
            "runs": self.runs,
            "landed": self.landed,
            "brokeAfterLanding": self.broke,
            "roundsPerLanded": rounds_per_landed,
            "progressTrips": self.trips,
            "landedWithCost": runs_with_cost,
            "medianCostUsd": eta::middle(self.cents).map(|cents| cents as f64 / 100.0),
            "medianMinutes": eta::middle(self.minutes),
        })
    }
}

fn how_far_it_goes(on: usize, off: usize) -> String {
    match (on, off) {
        (0, 0) => "No run has landed either way, so there is nothing to compare".to_owned(),
        (_, 0) => "No landed run had it off, so there is nothing to compare".to_owned(),
        (0, _) => "No landed run had it on, so there is nothing to compare".to_owned(),
        (on, off) if on < ENOUGH_LANDED || off < ENOUGH_LANDED => {
            format!("Thin: {on} landed with it on and {off} off, too few to go by")
        }
        (on, off) => format!("{on} landed with it on and {off} off"),
    }
}

fn case(card: &Card, record: &RunRecord) -> Value {
    let skipped = record.reviewer_found.len().saturating_sub(MAX_ASKED);
    let asked: Vec<String> = record.reviewer_found[skipped..]
        .iter()
        .map(|found| recover::clipped(found, MAX_ASKED_CHARS))
        .collect();
    json!({
        "card": card.id,
        "title": card.title,
        "reviewerApprovedSaying": record.approval_said,
        "reviewerAskedFor": asked,
    })
}

/// The latest runs that meet `read`, with how many did.
fn cases(runs: &[(&Card, &RunRecord)], read: impl Fn(&RunRecord) -> bool) -> Value {
    let mut found: Vec<&(&Card, &RunRecord)> =
        runs.iter().filter(|(_, record)| read(record)).collect();
    found.sort_by_key(|(_, record)| std::cmp::Reverse(record.started_at));
    let total = found.len();
    found.truncate(MAX_CASES);
    json!({
        "total": total,
        "latest": found.iter().map(|(card, record)| case(card, record)).collect::<Vec<_>>(),
    })
}

/// How the runs among `cards` compare with each switch on and off.
pub fn view<'a>(project: &str, cards: impl Iterator<Item = &'a Card>) -> Value {
    let runs: Vec<(&Card, &RunRecord)> = cards
        .filter_map(|card| Some((card, card.record.as_ref()?)))
        .collect();
    let mut by_switch: BTreeMap<&str, [Side; 2]> = BTreeMap::new();
    for (_, record) in &runs {
        for (switch, on) in &record.switches {
            by_switch.entry(switch).or_default()[usize::from(*on)].add(record);
        }
    }
    let switches: Vec<Value> = by_switch
        .into_iter()
        .map(|(switch, [off, on])| {
            json!({
                "switch": switch,
                "reads": how_far_it_goes(on.landed, off.landed),
                "on": on.view(),
                "off": off.view(),
            })
        })
        .collect();
    json!({
        "project": project,
        "runs": runs.len(),
        "switches": switches,
        "approvedThenBroke": cases(&runs, |record| {
            record.approved && record.broke_after_landing && record.finished_at.is_some()
        }),
        "overruled": cases(&runs, |record| record.overruled),
        "note": "Switches were not assigned at random, so a difference can come from which cards ran with them. Cost and time are medians of landed runs; progressTrips counts every run.",
    })
}
