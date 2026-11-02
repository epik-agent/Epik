use epik_ui::run_page::{Detail, Edges, Issue, RunView, shown_log};
use epik_view::{Change, Entry, Excerpt, LineRange, NodeState, Stream, StreamEntry};
use leptos::prelude::*;

mod fixtures;
use fixtures::{issue, link, render, run_page, session};

#[test]
fn an_issue_card_is_placed_by_its_column_and_row() {
    let html =
        render(|| view! { <Issue issue=issue(3, NodeState::NotStarted, 1, 1) chosen=false/> });
    assert!(html.starts_with(r##"<button title="#3 Add a power operator" class="issue not_started" style="left:256px;top:94px;width:200px;height:76px;">"##), "{html}");
    assert!(html.contains(r#"<div class="top"><span class="dot"></span><a href="https://github.com/acme/widgets/issues/3">#3</a></div>"#), "{html}");
    assert!(html.ends_with(r#"<div class="name">Add a power operator</div><span class="card-line">Starts when <a href="https://github.com/acme/widgets/issues/1">#1</a> is merged</span></button>"#), "{html}");
}

#[test]
fn a_merged_issue_card_has_its_figures() {
    let mut merged = issue(1, NodeState::Merged, 0, 0);
    merged.figures = Some("3m 05s · $0.92".into());
    let html = render(|| view! { <Issue issue=merged chosen=false/> });
    assert!(
        html.contains(r##"class="issue merged" style="left:0px;top:0px;"##),
        "{html}"
    );
    assert!(
        html.contains(r#"#1</a><span class="figures-small">3m 05s · $0.92</span></div>"#),
        "{html}"
    );
}

#[test]
fn a_running_issue_card_has_its_life_line_rather_than_its_figures() {
    let mut running = issue(2, NodeState::Running, 1, 0);
    running.figures = Some("1m 00s · $0.20".into());
    running.life = Some("active 2s ago".into());
    let html = render(|| view! { <Issue issue=running.clone() chosen=false/> });
    assert!(html.contains(r#"class="issue running""#), "{html}");
    assert!(
        html.contains(r#"#2</a><span class="life">active 2s ago</span></div>"#),
        "{html}"
    );
    assert!(!html.contains("figures-small"), "{html}");
    running.life = Some("quiet 1m 50s".into());
    let html = render(|| view! { <Issue issue=running chosen=false/> });
    assert!(
        html.contains(r#"<span class="life quiet">quiet 1m 50s</span>"#),
        "{html}"
    );
}

#[test]
fn a_failed_issue_card_is_failed() {
    let html = render(|| view! { <Issue issue=issue(2, NodeState::Failed, 1, 0) chosen=false/> });
    assert!(html.contains(r#"class="issue failed""#), "{html}");
}

#[test]
fn the_chosen_issue_card_is_chosen() {
    let html = render(|| view! { <Issue issue=issue(2, NodeState::Running, 1, 0) chosen=true/> });
    assert!(html.contains(r#"class="issue running chosen""#), "{html}");
}

#[test]
fn the_edges_join_a_blocker_to_what_it_blocks() {
    let page = run_page();
    let html = render(|| view! { <Edges issues=page.issues edges=page.edges/> });
    assert_eq!(
        html,
        concat!(
            r#"<path d="M200,38 C228,38 228,38 256,38" class="edge"></path>"#,
            r#"<path d="M200,38 C228,38 228,132 256,132" class="edge"></path>"#,
        )
    );
}

#[test]
fn an_edge_across_columns_bends_halfway() {
    let issues = vec![
        issue(1, NodeState::Merged, 0, 0),
        issue(2, NodeState::Running, 2, 0),
    ];
    let html = render(|| view! { <Edges issues edges=vec![(1, 2)]/> });
    assert_eq!(
        html,
        r#"<path d="M200,38 C356,38 356,38 512,38" class="edge"></path>"#
    );
}

#[test]
fn the_detail_panel_has_the_issues_facts_and_activity() {
    let mut running = issue(2, NodeState::Running, 1, 0);
    running.worked = Some("2m 40s · $0.48 so far".into());
    running.recent = vec!["Read src/lib.rs".into(), "Ran cargo test".into()];
    running.pull_request = vec![link("PR #31", "https://github.com/acme/widgets/pull/31")];
    let html = render(|| view! { <Detail issue=running summary=true/> });
    assert!(html.starts_with(r#"<section class="detail"><div class="facts"><h3><a href="https://github.com/acme/widgets/issues/2">#2</a> Add a power operator</h3><span class="state running">Waiting for #1</span>"#), "{html}");
    assert!(html.contains(r#"<dl><dt>Working for</dt><dd>2m 40s · $0.48 so far</dd><dt>Depends on</dt><dd><span><a href="https://github.com/acme/widgets/issues/1">#1</a></span></dd><dt>Unblocks</dt><dd><span>nothing</span></dd><dt>Pull request</dt><dd><span><a href="https://github.com/acme/widgets/pull/31">PR #31</a></span></dd></dl></div>"#), "{html}");
    assert!(html.ends_with(&(r#"<div class="doing">"#.to_owned() + SUMMARY_CHOSEN + r#"<ol class="recent"><li>Read src/lib.rs</li><li>Ran cargo test</li></ol></div></section>"#)), "{html}");
}

#[test]
fn the_detail_panel_of_an_ended_issue_says_how_long_it_took() {
    let mut merged = issue(1, NodeState::Merged, 0, 0);
    merged.worked = Some("3m 05s · $0.92".into());
    let html = render(|| view! { <Detail issue=merged/> });
    assert!(html.contains(r#"<span class="state merged">"#), "{html}");
    assert!(
        html.contains(r#"<dl><dt>Took</dt><dd>3m 05s · $0.92</dd>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<p class="muted nothing">Nothing yet.</p>"#),
        "{html}"
    );
}

#[test]
fn the_detail_panel_of_an_issue_not_started_has_no_time() {
    let html = render(|| view! { <Detail issue=issue(3, NodeState::NotStarted, 1, 1)/> });
    assert!(
        html.contains(r#"<span class="state not_started">"#),
        "{html}"
    );
    assert!(html.contains(r#"<dl><dt>Depends on</dt>"#), "{html}");
}

#[test]
fn a_runs_page_has_its_head_builds_graph_and_detail() {
    let mut page = run_page();
    page.builds.push("Last build, failed at 9:40".into());
    let html = render(|| view! { <RunView page chosen=Some(2)/> });
    assert!(html.starts_with(r##"<nav class="run-bar"><span class="crumbs"><a class="back" href="#/">‹ Runs</a><span class="slash">/</span><span>Exponentiation</span></span></nav><section class="run-head">"##), "{html}");
    assert!(html.contains(r#"<section class="run-head"><div><div class="about"><a href="https://github.com/acme/widgets/issues/13">acme/widgets#13</a> · <span>3 issues · merging into <code>epic/13-exponentiation</code></span></div><h1>Exponentiation</h1><p class="sentence building">First build running. 1 of 3 issues merged, 1 agent working.</p></div><dl class="totals"><dt>Elapsed</dt><dd>4m 12s</dd><dt>Cost so far</dt><dd>$1.20</dd></dl></section>"#), "{html}");
    assert!(html.contains(r#"<div class="builds"><span class="label">Builds</span><span class="build current">First build, started 10:02</span><span class="build">Last build, failed at 9:40</span></div>"#), "{html}");
    assert!(html.contains(r#"<div class="graph-scroll"><div class="graph" style="width:456px;height:170px;"><svg width="456" height="170"><path d="#), "{html}");
    assert_eq!(html.matches(r#"<button title="#).count(), 3, "{html}");
    assert!(html.contains(r#"class="issue running chosen""#), "{html}");
    assert!(html.ends_with(r#"</div></div></section>"#), "{html}");
    assert!(html.contains(r#"<section class="detail"><div class="facts"><h3><a href="https://github.com/acme/widgets/issues/2">#2</a>"#), "{html}");
}

#[test]
fn a_runs_page_without_a_chosen_issue_has_no_detail() {
    let html = render(|| view! { <RunView page=run_page() chosen=None/> });
    assert!(!html.contains("detail"), "{html}");
    assert!(!html.contains("chosen"), "{html}");
}

#[test]
fn a_run_needing_attention_says_so_in_red() {
    let mut page = run_page();
    page.disposition = epik_view::Disposition::NeedsAttention;
    page.sentence = "The build failed.".into();
    let html = render(|| view! { <RunView page chosen=None/> });
    assert!(
        html.contains(r#"<p class="sentence needs_attention">The build failed.</p>"#),
        "{html}"
    );
}

#[test]
fn a_runs_page_starts_with_its_breadcrumb_and_the_other_runs_building() {
    let mut page = run_page();
    page.also_building = vec![("acme/tools#2".into(), "Lint".into())];
    let html = render(|| view! { <RunView page chosen=None/> });
    assert!(html.starts_with(r##"<nav class="run-bar"><span class="crumbs"><a class="back" href="#/">‹ Runs</a><span class="slash">/</span><span>Exponentiation</span></span><span class="also">Also building: <a href="#/run/acme/tools%232">Lint</a></span></nav>"##), "{html}");
}

#[test]
fn the_detail_panel_draws_the_issue_on_the_left_and_its_agent_on_the_right() {
    let html = render(|| view! { <Detail issue=issue(2, NodeState::Running, 1, 0)/> });
    let facts = html.find(r#"<div class="facts">"#).expect(&html);
    let doing = html.find(r#"<div class="doing">"#).expect(&html);
    assert!(facts < doing, "{html}");
    assert!(html[facts..doing].contains("<h3>"), "{html}");
    assert!(html[facts..doing].contains("<dl>"), "{html}");
    assert!(
        html[doing..].contains(r#"<div class="stream-box">"#),
        "{html}"
    );
}

/// The switch with Full chosen, and with Summary chosen.
const FULL_CHOSEN: &str = r#"<div role="group" aria-label="What the agent has done" class="switch"><button aria-pressed="true">Full</button><button aria-pressed="false">Summary</button></div>"#;
const SUMMARY_CHOSEN: &str = r#"<div role="group" aria-label="What the agent has done" class="switch"><button aria-pressed="false">Full</button><button aria-pressed="true">Summary</button></div>"#;

/// 22:13:20 UTC, Nov 14, 2023.
const AT: i64 = 1_700_000_000_000;

fn stream(entries: Vec<Entry>, working: bool) -> Stream {
    Stream {
        entries: entries
            .into_iter()
            .enumerate()
            .map(|(i, what)| StreamEntry {
                at: Some(AT + 1000 * i as i64),
                what,
            })
            .collect(),
        working,
    }
}

fn lines(lines: &[&str]) -> Excerpt {
    Excerpt {
        head: lines.iter().map(|line| line.to_string()).collect(),
        more: 0,
        tail: vec![],
    }
}

/// The stream as the detail panel of a running issue draws it.
fn full(stream: Stream) -> String {
    let running = issue(2, NodeState::Running, 1, 0);
    let stream = Some(stream);
    render(move || view! { <Detail issue=running.clone() stream=stream.clone()/> })
}

#[test]
fn the_detail_panel_has_a_full_and_summary_switch_with_full_chosen() {
    let html = full(stream(vec![], false));
    assert!(
        html.contains(
            &(r#"<div class="doing">"#.to_owned() + FULL_CHOSEN + r#"<div class="stream-box">"#)
        ),
        "{html}"
    );
    assert!(!html.contains("What the agent has done</span>"), "{html}");
    assert!(!html.contains("Full log"), "{html}");
    assert!(!html.contains(r#"class="recent""#), "{html}");
}

#[test]
fn summary_shows_the_agents_recent_actions() {
    let mut running = issue(2, NodeState::Running, 1, 0);
    running.recent = vec!["Read src/lib.rs".into()];
    let stream = Some(stream(
        vec![Entry::Other {
            text: "Thinking".into(),
        }],
        true,
    ));
    let html = render(
        move || view! { <Detail issue=running.clone() stream=stream.clone() summary=true/> },
    );
    assert!(
        html.contains(
            &(SUMMARY_CHOSEN.to_owned() + r#"<ol class="recent"><li>Read src/lib.rs</li></ol>"#)
        ),
        "{html}"
    );
    assert!(!html.contains("Thinking"), "{html}");
    assert!(!html.contains("cursor"), "{html}");
}

#[test]
fn without_a_log_the_stream_says_nothing_yet() {
    let html = render(|| view! { <Detail issue=issue(2, NodeState::Running, 1, 0)/> });
    assert!(html.contains(r#"<div class="stream-box"><div class="stream"><p class="muted nothing">Nothing yet.</p></div></div>"#), "{html}");
}

#[test]
fn narration_is_plain_sentences_with_code_in_monospace() {
    let html = full(stream(
        vec![Entry::Narration {
            text: "Two failures, both in `upload::tests`.\n\nChecking `put`.".into(),
        }],
        false,
    ));
    assert!(html.contains("<div class=\"stream\"><div class=\"entry narration\"><time>22:13:20</time><div class=\"what\"><p>Two failures, both in <code>upload::tests</code>.</p>\n<p>Checking <code>put</code>.</p>\n</div></div>"), "{html}");
}

/// What the narration `text` is drawn as.
fn narrated(text: &str) -> String {
    let html = full(stream(vec![Entry::Narration { text: text.into() }], false));
    let start =
        html.find(r#"<div class="what">"#).expect("an entry") + r#"<div class="what">"#.len();
    let end = start + html[start..].find("</div>").expect("the end of the entry");
    html[start..end].to_owned()
}

#[test]
fn narrated_code_blocks_are_kept_whole() {
    assert_eq!(
        narrated("The fix:\n\n```rust\nlet x = `a`;\n```\n\nDone."),
        "<p>The fix:</p>\n<pre><code class=\"language-rust\">let x = `a`;\n</code></pre>\n<p>Done.</p>\n"
    );
}

#[test]
fn a_stray_backtick_is_just_a_backtick() {
    assert_eq!(
        narrated("Renamed `foo to bar, keeping `baz`."),
        "<p>Renamed <code>foo to bar, keeping </code>baz`.</p>\n"
    );
    assert_eq!(
        narrated("Renamed `foo to bar."),
        "<p>Renamed `foo to bar.</p>\n"
    );
}

#[test]
fn html_in_narration_is_shown_not_run() {
    assert_eq!(
        narrated("Removed <script>alert(1)</script> from <b>it</b>."),
        "<p>Removed &lt;script&gt;alert(1)&lt;/script&gt; from &lt;b&gt;it&lt;/b&gt;.</p>\n"
    );
}

#[test]
fn narrated_links_are_their_text() {
    assert_eq!(
        narrated("See [the docs](https://example.com) and ![a chart](chart.png)."),
        "<p>See the docs and a chart.</p>\n"
    );
}

#[test]
fn an_entry_without_a_time_leaves_its_time_empty() {
    let html = full(Stream {
        entries: vec![StreamEntry {
            at: None,
            what: Entry::Other {
                text: "Updating its plan".into(),
            },
        }],
        working: false,
    });
    assert!(html.contains(r#"<div class="entry other"><time></time><div class="what">Updating its plan</div></div>"#), "{html}");
}

#[test]
fn a_command_shows_its_line_and_output_with_failures_in_red() {
    let html = full(stream(
        vec![Entry::Command {
            command: "cargo test".into(),
            output: Excerpt {
                head: vec!["running 43 tests".into(), "test a ... ok".into()],
                more: 40,
                tail: vec![
                    "assertion `left == right` failed".into(),
                    "  left: 1".into(),
                    "test result: FAILED. 42 passed; 1 failed; 0 ignored".into(),
                ],
            },
        }],
        false,
    ));
    assert!(
        html.contains(concat!(
            r#"<div class="entry command"><time>22:13:20</time><div class="what">"#,
            r#"<div class="command-line"><span class="prompt">$ </span>cargo test</div>"#,
            r#"<div class="output"><div>running 43 tests</div><div>test a ... ok</div>"#,
            r#"<div class="more">… 40 more lines</div>"#,
            r#"<div class="failed">assertion `left == right` failed</div><div>  left: 1</div>"#,
            r#"<div class="failed">test result: FAILED. 42 passed; 1 failed; 0 ignored</div>"#,
            r#"</div></div></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn a_command_without_output_is_its_line() {
    let html = full(stream(
        vec![Entry::Command {
            command: "git status".into(),
            output: Excerpt::default(),
        }],
        false,
    ));
    assert!(html.contains(r#"<div class="what"><div class="command-line"><span class="prompt">$ </span>git status</div></div>"#), "{html}");
}

#[test]
fn one_more_line_is_singular() {
    let html = full(stream(
        vec![Entry::Command {
            command: "ls".into(),
            output: Excerpt {
                head: vec!["a".into()],
                more: 1,
                tail: vec!["c".into()],
            },
        }],
        false,
    ));
    assert!(
        html.contains(r#"<div class="more">… 1 more line</div>"#),
        "{html}"
    );
}

#[test]
fn reads_are_one_line_with_their_range() {
    let read = |lines| Entry::Read {
        path: "src/upload.rs".into(),
        lines,
    };
    let html = full(stream(
        vec![
            read(None),
            read(Some(LineRange {
                from: 30,
                to: Some(70),
            })),
            read(Some(LineRange { from: 30, to: None })),
        ],
        false,
    ));
    assert!(html.contains(r#"<div class="entry read"><time>22:13:20</time><div class="what">Read <code>src/upload.rs</code></div></div>"#), "{html}");
    assert!(
        html.contains(r#"<div class="what">Read <code>src/upload.rs</code> (lines 30-70)</div>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="what">Read <code>src/upload.rs</code> (from line 30)</div>"#),
        "{html}"
    );
}

#[test]
fn searches_are_one_line_with_their_matches() {
    let html = full(stream(
        vec![
            Entry::Search {
                pattern: "fn put".into(),
                within: Some("crates/epik-cache/src".into()),
                matches: 2,
                hits: lines(&["store.rs:88", "upload.rs:41"]),
            },
            Entry::Search {
                pattern: "TODO".into(),
                within: None,
                matches: 0,
                hits: Excerpt::default(),
            },
            Entry::Search {
                pattern: "*.rs".into(),
                within: None,
                matches: 9,
                hits: Excerpt {
                    head: vec!["a.rs".into()],
                    more: 7,
                    tail: vec!["z.rs".into()],
                },
            },
        ],
        false,
    ));
    assert!(html.contains(r#"<div class="entry search"><time>22:13:20</time><div class="what">Searched for <code>"fn put"</code> in <code>crates/epik-cache/src</code> · 2 matches: store.rs:88, upload.rs:41</div></div>"#), "{html}");
    assert!(
        html.contains(r#"<div class="what">Searched for <code>"TODO"</code> · no matches</div>"#),
        "{html}"
    );
    assert!(html.contains(r#"<div class="what">Searched for <code>"*.rs"</code> · 9 matches: a.rs, … 7 more, z.rs</div>"#), "{html}");
}

#[test]
fn an_edit_shows_removed_lines_in_red_and_added_lines_in_green() {
    let html = full(stream(
        vec![Entry::Edit {
            path: "src/upload.rs".into(),
            changes: Excerpt {
                head: vec![
                    Change::Removed("entry.path".into()),
                    Change::Added("entry.digest".into()),
                ],
                more: 3,
                tail: vec![Change::Added("}".into())],
            },
        }],
        false,
    ));
    assert!(html.contains(concat!(
        r#"<div class="entry edit"><time>22:13:20</time><div class="what">"#,
        r#"<div class="path">Edited <code>src/upload.rs</code></div>"#,
        r#"<div class="output"><div class="removed">- entry.path</div><div class="added">+ entry.digest</div>"#,
        r#"<div class="more">… 3 more lines</div><div class="added">+ }</div></div>"#,
        r#"</div></div>"#,
    )), "{html}");
}

#[test]
fn a_new_file_shows_its_first_lines() {
    let html = full(stream(
        vec![Entry::NewFile {
            path: "tests/upload.rs".into(),
            content: Excerpt {
                head: vec!["use epik_cache::Store;".into()],
                more: 20,
                tail: vec!["}".into()],
            },
        }],
        false,
    ));
    assert!(html.contains(concat!(
        r#"<div class="entry new_file"><time>22:13:20</time><div class="what">"#,
        r#"<div class="path">Created <code>tests/upload.rs</code></div>"#,
        r#"<div class="output"><div>use epik_cache::Store;</div><div class="more">… 20 more lines</div><div>}</div></div>"#,
        r#"</div></div>"#,
    )), "{html}");
}

#[test]
fn a_blinking_cursor_shows_the_agent_is_still_working() {
    let entries = || {
        vec![Entry::Other {
            text: "Thinking".into(),
        }]
    };
    let html = full(stream(entries(), true));
    assert!(
        html.contains(
            r#"<div class="what">Thinking</div></div><span class="cursor"></span></div>"#
        ),
        "{html}"
    );
    let html = full(stream(entries(), false));
    assert!(!html.contains("cursor"), "{html}");
}

#[test]
fn a_runs_page_streams_the_chosen_issues_log() {
    let mut page = run_page();
    let path = "/Users/me/.epik/runs/acme/widgets/13/1/2.jsonl".to_owned();
    for issue in &mut page.issues {
        if issue.number == 2 {
            issue.log = Some(path.clone());
        }
    }
    let streamed = Some((
        path,
        stream(
            vec![Entry::Other {
                text: "Thinking".into(),
            }],
            false,
        ),
    ));
    let html = render({
        let (page, streamed) = (page.clone(), streamed.clone());
        move || view! { <RunView page=page.clone() chosen=Some(2) streamed=streamed.clone()/> }
    });
    assert!(
        html.contains(r#"<div class="what">Thinking</div>"#),
        "{html}"
    );
    // Another issue's stream isn't drawn.
    let html = render(
        move || view! { <RunView page=page.clone() chosen=Some(3) streamed=streamed.clone()/> },
    );
    assert!(!html.contains("Thinking"), "{html}");
    assert!(html.contains("Nothing yet."), "{html}");
}

#[test]
fn a_hosts_runs_page_says_where_it_builds_under_its_sentence() {
    let mut page = run_page();
    page.run = "devbox:acme/widgets#13".into();
    page.place = Some("Building on devbox".into());
    let html = render(|| view! { <RunView page chosen=None/> });
    assert!(
        html.contains(r#"</p><p class="place">Building on devbox</p></div>"#),
        "{html}"
    );
}

#[test]
fn a_runs_page_with_nowhere_to_say_says_nothing_of_it() {
    let html = render(|| view! { <RunView page=run_page() chosen=None/> });
    assert!(!html.contains(r#"class="place""#), "{html}");
}

#[test]
fn another_run_building_on_a_host_is_linked_by_its_host() {
    let mut page = run_page();
    page.also_building = vec![("devbox:acme/tools#2".into(), "Lint".into())];
    let html = render(|| view! { <RunView page chosen=None/> });
    assert!(
        html.contains(r##"<a href="#/run/devbox:acme/tools%232">Lint</a>"##),
        "{html}"
    );
}

#[test]
fn the_detail_panel_lists_the_issues_sessions_in_order() {
    let mut card = issue(2, NodeState::Merged, 1, 0);
    card.sessions = vec![
        session("Coding", 1, None, "/logs/2.jsonl"),
        session(
            "Review",
            1,
            Some("changes requested"),
            "/logs/2-review-1.jsonl",
        ),
    ];
    let html =
        render(|| view! { <Detail issue=card shown=Some("/logs/2-review-1.jsonl".to_owned())/> });
    assert!(html.contains(concat!(
        r#"<ol class="sessions">"#,
        r#"<li class="session"><button aria-pressed="false"><span class="what">Coding · round 1</span><span class="who">Claude Code · default model</span><span class="figures">1m 00s · $0.30</span></button></li>"#,
        r#"<li class="session"><button aria-pressed="true"><span class="what">Review · round 1</span><span class="who">Claude Code · default model</span><span class="figures">1m 00s · $0.30</span></button><span class="verdict">changes requested</span></li>"#,
        "</ol>",
    )), "{html}");
}

#[test]
fn a_running_session_has_its_time_so_far_and_no_cost_yet() {
    let mut card = issue(2, NodeState::Running, 1, 0);
    let mut row = session("Coding", 1, None, "/logs/2.jsonl");
    row.time = "2m 00s so far".into();
    row.cost = None;
    row.running = true;
    card.sessions = vec![row];
    let html = render(|| view! { <Detail issue=card/> });
    assert!(html.contains(r#"<li class="session running"><button aria-pressed="false"><span class="what">Coding · round 1</span><span class="who">Claude Code · default model</span><span class="figures">2m 00s so far</span></button></li>"#), "{html}");
}

#[test]
fn a_codex_session_that_reported_no_cost_says_not_reported() {
    let mut card = issue(2, NodeState::Merged, 1, 0);
    let mut row = session("Coding", 1, None, "/logs/2.jsonl");
    row.agent = "Codex".into();
    row.model = "gpt-5.5".into();
    row.cost = Some("not reported".into());
    card.sessions = vec![row];
    let html = render(|| view! { <Detail issue=card/> });
    assert!(html.contains(r#"<span class="who">Codex · gpt-5.5</span><span class="figures">1m 00s · not reported</span>"#), "{html}");
}

/// The run page of a run reviewed twice, the fix between, with two threads
/// left open.
fn reviewed() -> epik_view::RunPage {
    let mut page = run_page();
    page.disposition = epik_view::Disposition::WaitingForReview;
    let mut last = session("Review", 2, Some("approved"), "/logs/13-review-2.jsonl");
    last.threads = Some(link(
        "2 threads open",
        "https://github.com/acme/widgets/pull/30/files",
    ));
    page.sessions = vec![
        session(
            "Review",
            1,
            Some("changes requested"),
            "/logs/13-review-1.jsonl",
        ),
        session("Coding", 2, None, "/logs/13-coding-2.jsonl"),
        last,
    ];
    page
}

#[test]
fn a_reviewed_runs_sessions_are_listed_in_order_under_its_header() {
    let page = reviewed();
    let html = render(|| view! { <RunView page chosen=None/> });
    let head = html
        .find(r#"</section><div class="run-sessions">"#)
        .expect(&html);
    let builds = html.find(r#"<div class="builds">"#).expect(&html);
    assert!(head < builds, "{html}");
    let sessions = &html[head..builds];
    let review_1 = sessions.find("Review · round 1").expect(&html);
    let fix = sessions.find("Coding · round 2").expect(&html);
    let review_2 = sessions.find("Review · round 2").expect(&html);
    assert!(review_1 < fix && fix < review_2, "{html}");
    assert!(
        sessions.contains(r#"<span class="verdict">changes requested</span>"#),
        "{html}"
    );
    assert!(sessions.ends_with(r#"</button><span class="verdict">approved</span><span class="threads"><a href="https://github.com/acme/widgets/pull/30/files">2 threads open</a></span></li></ol></div>"#), "{html}");
}

#[test]
fn a_run_with_no_sessions_of_its_own_lists_none() {
    let html = render(|| view! { <RunView page=run_page() chosen=None/> });
    assert!(!html.contains("run-sessions"), "{html}");
}

#[test]
fn a_reviewing_run_says_so_in_its_header() {
    let mut page = run_page();
    page.disposition = epik_view::Disposition::Reviewing;
    page.sentence = "First build running, reviewing its pull request.".into();
    let html = render(|| view! { <RunView page chosen=None/> });
    assert!(
        html.contains(
            r#"<p class="sentence building">First build running, reviewing its pull request.</p>"#
        ),
        "{html}"
    );
}

#[test]
fn the_stream_shows_the_chosen_issues_latest_session_by_default() {
    let mut page = reviewed();
    page.issues[1].log = Some("/logs/2.jsonl".into());
    assert_eq!(
        shown_log(&page, Some(2), None).as_deref(),
        Some("/logs/2.jsonl")
    );
    assert_eq!(shown_log(&page, None, None), None);
}

#[test]
fn the_stream_shows_the_session_picked_of_the_issue_or_the_run() {
    let mut page = reviewed();
    page.issues[1].log = Some("/logs/2-coding-2.jsonl".into());
    page.issues[1].sessions = vec![
        session("Coding", 1, None, "/logs/2.jsonl"),
        session("Coding", 2, None, "/logs/2-coding-2.jsonl"),
    ];
    let shown = |picked| shown_log(&page, Some(2), Some(picked));
    assert_eq!(shown("/logs/2.jsonl").as_deref(), Some("/logs/2.jsonl"));
    assert_eq!(
        shown("/logs/13-review-1.jsonl").as_deref(),
        Some("/logs/13-review-1.jsonl")
    );
    // A session no longer on the page, such as another issue's.
    assert_eq!(
        shown("/logs/3.jsonl").as_deref(),
        Some("/logs/2-coding-2.jsonl")
    );
}

#[test]
fn while_the_run_reviews_the_stream_shows_the_review() {
    let mut page = run_page();
    let mut review = session("Review", 1, None, "/logs/13-review-1.jsonl");
    review.running = true;
    page.sessions = vec![review];
    page.issues[1].log = Some("/logs/2.jsonl".into());
    assert_eq!(
        shown_log(&page, Some(2), None).as_deref(),
        Some("/logs/13-review-1.jsonl")
    );
}

#[test]
fn a_runs_page_streams_the_session_picked() {
    let page = reviewed();
    let path = "/logs/13-review-1.jsonl".to_owned();
    let streamed = Some((
        path.clone(),
        stream(
            vec![Entry::Other {
                text: "Reviewing".into(),
            }],
            false,
        ),
    ));
    let html = render(move || {
        view! { <RunView page chosen=Some(2) picked=Some(path) streamed/> }
    });
    assert!(
        html.contains(r#"<div class="what">Reviewing</div>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<button aria-pressed="true"><span class="what">Review · round 1</span>"#),
        "{html}"
    );
}
