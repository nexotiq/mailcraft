// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #143: mail older than the mail to keep offline left on the server. Each folder's round is begun
//! from the window's start; a round begun from another time is begun afresh; and what is older
//! than the window goes at the end of a whole run, but for what has a change waiting to go up.

use mailcraft_model::sync::{GraphFolder, GraphMailResult, GraphMessage, MessagePage, ServerMessage};
use mailcraft_model::{FolderKind, KeepOffline};
use serde_json::json;

use super::*;

const ACCOUNT_EMAIL: &str = "owner@example.org";
/// 2026-09-21 14:13:20 UTC.
const T: i64 = 1_790_000_000;
const DAY: i64 = 86_400;

/// A session with one Microsoft account (mail to keep offline: 12 months) whose Inbox, Drafts and
/// Projects were listed once, with the clock at `T`.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.clock.now = Some(T);
    let added = s.run("account.add", &json!({"email": ACCOUNT_EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders =
        vec![folder("in", "Inbox", FolderKind::Inbox), folder("dr", "Drafts", FolderKind::Drafts), folder("pr", "Projects", FolderKind::Custom)];
    take(&mut s, account, GraphMailResult { listing: Some(FolderListing { folders, complete: true }), partial: true, ..Default::default() });
    (s, account)
}

fn take(s: &mut Session, account: Id, mut result: GraphMailResult) -> Value {
    result.account = account;
    result.email = ACCOUNT_EMAIL.into();
    apply_graph_mail(s, result).unwrap()
}

fn turn_on(s: &mut Session, account: Id) {
    s.run("account.edit", &json!({"id": account, "olderOnServer": true})).unwrap();
}

/// 00:00 UTC of the day 12 months before `T`.
fn window_start() -> i64 {
    let since = KeepOffline::Months12.since(T).unwrap();
    since - since.rem_euclid(DAY)
}

/// A page that ends a round of `folder` begun from `since`, with `messages`.
fn round(folder: &str, since: Option<i64>, messages: Vec<GraphMessage>, afresh: bool) -> MessagePage {
    MessagePage {
        folder: folder.into(),
        messages,
        link: format!("https://graph.microsoft.com/v1.0/me/mailFolders/{folder}/messages/delta?$deltatoken=1"),
        caught_up: true,
        afresh,
        since,
        ..Default::default()
    }
}

fn message(id: &str, received: i64) -> GraphMessage {
    GraphMessage { server: ServerMessage { id: id.into(), ..Default::default() }, subject: format!("Message {id}"), received, ..Default::default() }
}

fn link_of<'r>(request: &'r GraphMailRequest, folder: &str) -> Option<&'r GraphLink> {
    request.links.iter().find(|l| l.folder == folder)
}

fn here(s: &Session, account: Id, id: &str) -> Option<Id> {
    s.mailbox.messages.values().find(|m| m.account == account && m.server.as_ref().is_some_and(|sv| sv.id == id)).map(|m| m.id)
}

#[test]
fn off_lists_everything_and_on_lists_from_the_start_of_the_window_day() {
    let (mut s, account) = session();
    assert_eq!(graph_mail_request(&s, account).unwrap().since, None);
    turn_on(&mut s, account);
    let since = graph_mail_request(&s, account).unwrap().since.unwrap();
    assert_eq!(since, window_start());
    assert_eq!(since % DAY, 0, "00:00 UTC");
    assert!(since <= KeepOffline::Months12.since(T).unwrap());
    // All: nothing to leave.
    s.run("account.edit", &json!({"id": account, "keepOffline": "all"})).unwrap();
    assert_eq!(graph_mail_request(&s, account).unwrap().since, None);
    let listed = s.run("account.list", &json!({})).unwrap();
    assert_eq!(listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == json!(account)).unwrap()["olderOnServer"], true);
}

#[test]
fn a_round_begun_from_another_time_is_begun_afresh_and_one_from_the_same_goes_on() {
    let (mut s, account) = session();
    // Read once, everything listed.
    for f in ["in", "dr", "pr"] {
        take(&mut s, account, GraphMailResult { pages: vec![round(f, None, Vec::new(), false)], partial: true, ..Default::default() });
    }
    let request = graph_mail_request(&s, account).unwrap();
    assert!(request.links.iter().all(|l| !l.link.is_empty()), "off: every link goes on");
    turn_on(&mut s, account);
    let request = graph_mail_request(&s, account).unwrap();
    // The Inbox and Projects start afresh (a link that is none of Graph's); the Drafts list
    // everything still, and go on.
    assert_eq!(link_of(&request, "in").unwrap().link, "");
    assert_eq!(link_of(&request, "pr").unwrap().link, "");
    assert!(!link_of(&request, "dr").unwrap().link.is_empty());
    assert!(graph_inbox_request(&s, account).is_none(), "no Inbox pass from a link that doesn't fit");
    // Begun afresh from the window's start: that link goes on.
    let since = Some(window_start());
    take(&mut s, account, GraphMailResult { pages: vec![round("in", since, Vec::new(), true)], partial: true, ..Default::default() });
    let request = graph_mail_request(&s, account).unwrap();
    assert_eq!(link_of(&request, "in").unwrap().since, since);
    assert!(!link_of(&request, "in").unwrap().link.is_empty());
    assert!(graph_inbox_request(&s, account).is_some());
    // A month on, the window starts later: a round from earlier still serves.
    s.clock.now = Some(T + 31 * DAY);
    assert!(!link_of(&graph_mail_request(&s, account).unwrap(), "in").unwrap().link.is_empty());
    // A longer window starts earlier: begun afresh. So is turning it off.
    s.run("account.edit", &json!({"id": account, "keepOffline": "2y"})).unwrap();
    assert_eq!(link_of(&graph_mail_request(&s, account).unwrap(), "in").unwrap().link, "");
    s.run("account.edit", &json!({"id": account, "keepOffline": "12m", "olderOnServer": false})).unwrap();
    assert_eq!(link_of(&graph_mail_request(&s, account).unwrap(), "in").unwrap().link, "");
}

#[test]
fn older_mail_goes_at_the_end_of_a_whole_run_but_not_what_waits_to_go_up() {
    let (mut s, account) = session();
    let start = window_start();
    let old = |id: &str| message(id, start - 10 * DAY);
    // Listed before the switch: everything.
    let pages = vec![
        round("in", None, vec![message("new", T - DAY), old("old"), old("read-here"), old("moved-here")], false),
        round("dr", None, vec![old("draft")], false),
        round("pr", None, vec![old("project")], false),
    ];
    take(&mut s, account, GraphMailResult { pages, whole: true, ..Default::default() });
    assert_eq!(s.mailbox.messages.len(), 6);
    // Changes made here and not gone up yet: one read, one moved to Projects.
    let read_here = here(&s, account, "read-here").unwrap();
    s.mailbox.message_mut(read_here).unwrap().read = true;
    let moved = here(&s, account, "moved-here").unwrap();
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.mailbox.message_mut(moved).unwrap().folder = projects;
    turn_on(&mut s, account);
    // The Inbox read afresh from the window's start lists only what is within; Projects too.
    let since = Some(start);
    let pages = vec![round("in", since, vec![message("new", T - DAY)], true), round("pr", since, Vec::new(), true)];
    take(&mut s, account, GraphMailResult { pages, whole: true, ..Default::default() });
    assert!(here(&s, account, "new").is_some());
    assert!(here(&s, account, "old").is_none(), "older: left on the server");
    assert!(here(&s, account, "project").is_none());
    assert!(here(&s, account, "draft").is_some(), "the Drafts list everything");
    let kept = |id: &str| here(&s, account, id).and_then(|m| s.mailbox.message(m)).and_then(|m| m.server.clone()).is_some_and(|sv| !sv.gone);
    assert!(kept("read-here"), "its change hasn't gone up");
    assert!(kept("moved-here"), "its move hasn't gone up");
    // Once the read state is the server's, it goes at the next whole run.
    let m = s.mailbox.message_mut(read_here).unwrap();
    m.server.as_mut().unwrap().read = true;
    take(&mut s, account, GraphMailResult { whole: true, ..Default::default() });
    assert!(here(&s, account, "read-here").is_none());
    assert!(here(&s, account, "moved-here").is_some());
}

#[test]
fn a_shorter_window_and_the_days_going_by_leave_more_on_the_server_without_reading_afresh() {
    let (mut s, account) = session();
    turn_on(&mut s, account);
    let since = Some(window_start());
    let pages = vec![round("in", since, vec![message("week", T - 7 * DAY), message("months", T - 120 * DAY)], true)];
    take(&mut s, account, GraphMailResult { pages, whole: true, ..Default::default() });
    assert_eq!(s.mailbox.messages.len(), 2);
    // Three months: the round from 12 months back goes on, and what is older goes.
    s.run("account.edit", &json!({"id": account, "keepOffline": "3m"})).unwrap();
    assert!(!link_of(&graph_mail_request(&s, account).unwrap(), "in").unwrap().link.is_empty());
    take(&mut s, account, GraphMailResult { whole: true, ..Default::default() });
    assert!(here(&s, account, "months").is_none());
    assert!(here(&s, account, "week").is_some());
    // Three months on, the week-old message is older than the window too.
    s.clock.now = Some(T + 100 * DAY);
    take(&mut s, account, GraphMailResult { whole: true, ..Default::default() });
    assert!(here(&s, account, "week").is_none());
}

#[test]
fn nothing_goes_after_a_run_that_wasnt_whole() {
    let (mut s, account) = session();
    turn_on(&mut s, account);
    let since = Some(window_start());
    take(&mut s, account, GraphMailResult { pages: vec![round("in", since, vec![message("x", T)], true)], whole: true, ..Default::default() });
    s.run("account.edit", &json!({"id": account, "keepOffline": "1w"})).unwrap();
    s.clock.now = Some(T + 30 * DAY);
    take(&mut s, account, GraphMailResult { stopped: true, ..Default::default() });
    assert!(here(&s, account, "x").is_some());
}

#[test]
fn a_shared_mailbox_follows_its_owner() {
    let (mut s, account) = session();
    let shared = s.run("account.addSharedMailbox", &json!({"account": account, "address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("account.edit", &json!({"id": shared, "olderOnServer": true})).is_err());
    assert_eq!(s.mailbox.listed_since(shared, T), None);
    turn_on(&mut s, account);
    assert_eq!(s.mailbox.listed_since(shared, T), Some(window_start()));
    assert!(s.run("account.edit", &json!({"id": account, "olderOnServer": "yes"})).is_err());
}

#[test]
fn a_round_fits_only_one_begun_from_the_same_time_or_earlier() {
    assert!(fits(None, None));
    assert!(fits(Some(5), Some(5)));
    assert!(fits(Some(4), Some(5)));
    assert!(!fits(Some(6), Some(5)));
    assert!(!fits(None, Some(5)));
    assert!(!fits(Some(5), None));
}
