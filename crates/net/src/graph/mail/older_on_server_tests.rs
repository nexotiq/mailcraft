// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #143: a folder's round begun from a received time (`$filter=receivedDateTime ge`), through the
//! strict Graph stand-in. Nothing here leaves this machine; every message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{GraphLink, GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
/// 2026-09-21 14:13:20 UTC.
const NOW: i64 = 1_790_000_000;
const DAY: i64 = 86_400;
/// 2026-09-01 00:00 UTC.
const SINCE: i64 = 1_788_220_800;

fn run(server: &StandIn, links: Vec<GraphLink>, since: Option<i64>) -> Vec<GraphMailResult> {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), links, since, ..Default::default() };
    whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

fn links_after(pieces: &[GraphMailResult], mut links: Vec<GraphLink>) -> Vec<GraphLink> {
    for page in pieces.iter().flat_map(|p| &p.pages) {
        links.retain(|l| l.folder != page.folder);
        links.push(GraphLink {
            folder: page.folder.clone(),
            link: page.link.clone(),
            caught_up: page.caught_up,
            counts: page.counts,
            since: page.since,
        });
    }
    links
}

fn subjects_of(pieces: &[GraphMailResult], folder: &str) -> Vec<String> {
    let mut all: Vec<String> =
        pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == folder).flat_map(|p| &p.messages).map(|m| m.subject.clone()).collect();
    all.sort();
    all
}

/// The `$filter` of every delta request for `folder` from request number `from` on.
fn filters_of(server: &StandIn, folder: &str, from: usize) -> Vec<Option<String>> {
    let state = server.state();
    state.requests[from..]
        .iter()
        .filter(|r| r.path.last().is_some_and(|p| p == "delta") && r.path.iter().any(|p| p == folder))
        .map(|r| r.query.iter().find(|(name, _)| name == "$filter").map(|(_, value)| value.clone()))
        .collect()
}

#[test]
fn a_round_from_a_time_lists_only_what_came_since_and_goes_on_from_its_link() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let drafts = server.state().well_known("drafts");
    {
        let mut state = server.state();
        state.mailbox.add(&inbox, "This week", NOW - 2 * DAY);
        state.mailbox.add(&inbox, "On the day", SINCE);
        state.mailbox.add(&inbox, "Last year", NOW - 400 * DAY);
        state.mailbox.add(&drafts, "Old draft", NOW - 400 * DAY);
    }
    let pieces = run(&server, Vec::new(), Some(SINCE));
    let last = pieces.last().unwrap();
    assert!(last.whole && last.errors.is_empty(), "{:?}", last.errors);
    assert_eq!(subjects_of(&pieces, &inbox), ["On the day", "This week"]);
    // The Drafts list everything, as the mail to keep offline keeps them all.
    assert_eq!(subjects_of(&pieces, &drafts), ["Old draft"]);
    assert_eq!(filters_of(&server, &inbox, 0), [Some("receivedDateTime ge 2026-09-01T00:00:00Z".to_string())]);
    assert_eq!(filters_of(&server, &drafts, 0), [None]);
    // Every page says the time its round was begun from.
    for page in pieces.iter().flat_map(|p| &p.pages) {
        assert_eq!(page.since, if page.folder == drafts { None } else { Some(SINCE) }, "{}", page.folder);
    }
    // From the delta link: what is new, within the filter, and the link's time kept.
    let links = links_after(&pieces, Vec::new());
    let from = server.state().requests.len();
    server.state().mailbox.add(&inbox, "Today", NOW);
    server.state().mailbox.add(&inbox, "Moved in, old", NOW - 500 * DAY);
    let pieces = run(&server, links, Some(SINCE + DAY));
    assert_eq!(subjects_of(&pieces, &inbox), ["Today"]);
    assert_eq!(filters_of(&server, &inbox, from), [None], "the link carries the rest");
    assert!(pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).all(|p| p.since == Some(SINCE)));
}

#[test]
fn a_link_no_longer_taken_begins_again_from_the_time_asked_for_now() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    {
        let mut state = server.state();
        state.mailbox.add(&inbox, "New", NOW);
        state.mailbox.add(&inbox, "Old", NOW - 400 * DAY);
    }
    let links = links_after(&run(&server, Vec::new(), None), Vec::new());
    server.state().mailbox.forget_links();
    let pieces = run(&server, links, Some(SINCE));
    assert_eq!(subjects_of(&pieces, &inbox), ["New"]);
    let page = pieces.iter().flat_map(|p| &p.pages).find(|p| p.folder == inbox).unwrap();
    assert!(page.afresh);
    assert_eq!(page.since, Some(SINCE));
}

#[test]
fn a_link_that_is_none_of_graphs_begins_afresh_from_the_time() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.add(&inbox, "New", NOW);
    server.state().mailbox.add(&inbox, "Old", NOW - 400 * DAY);
    // As the engine sends a folder whose round was begun from another time.
    let pieces = run(&server, vec![GraphLink { folder: inbox.clone(), ..Default::default() }], Some(SINCE));
    let page = pieces.iter().flat_map(|p| &p.pages).find(|p| p.folder == inbox).unwrap();
    assert!(page.afresh && page.since == Some(SINCE));
    assert_eq!(subjects_of(&pieces, &inbox), ["New"]);
}
