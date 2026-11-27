//! The season console (a season's admin page).

use super::*;

/// The "Signed up" roster of submissions, with a Remove control on your own (or,
/// for an admin, any) entry — posts to `/signup/delete`.
pub(super) fn roster(subs: &[SubmissionRow], me: &str, is_admin: bool, pre_gen: bool) -> String {
    let mut b = String::from(r#"<h2>Signed up</h2>"#);
    if subs.is_empty() {
        b.push_str(r#"<div class="empty">Nobody yet.</div>"#);
        return b;
    }
    b.push_str(r#"<div class="grid">"#);
    for s in subs {
        let custom = if s.is_custom {
            r#" <span class="by free">custom</span>"#
        } else {
            ""
        };
        let held = match (&s.held_for, &s.held_err) {
            (Some(_), Some(_)) if !(is_admin || s.username == me) => {
                r#"<div class="meta">held</div>"#.to_string()
            }
            (Some(_), Some(e)) => format!(
                r#"<div class="meta" style="color:var(--bad)">held: {}</div>"#,
                esc(e)
            ),
            (Some(w), None) => format!(r#"<div class="meta">held, waiting for {}</div>"#, esc(w)),
            _ => String::new(),
        };
        let del = if pre_gen && (is_admin || s.username == me) {
            format!(
                r#"<div class="seat"><span></span><form method="post" action="signup/delete" style="display:inline">
<input type="hidden" name="id" value="{}"><button class="ghost" type="submit">Remove</button></form></div>"#,
                s.id
            )
        } else {
            String::new()
        };
        let seats = if s.seat_names.is_empty() {
            String::new()
        } else {
            format!(r#"<div class="meta">+ {}</div>"#, esc(&s.seat_names))
        };
        let notes = if s.notes.is_empty() {
            String::new()
        } else {
            format!(r#"<div class="notes">{}</div>"#, esc(&s.notes))
        };
        b.push_str(&format!(
            r#"<div class="slot idle stretch"><div class="game"><a class="cover" href="signup/game/{}">{}</a>{custom}</div>
<div class="meta">{} · by {}</div>{held}{seats}{notes}{del}</div>"#,
            s.id,
            esc(&s.slot_name),
            esc(&s.game),
            esc(&s.username),
        ));
    }
    b.push_str("</div>");
    b
}

/// A one-field POST form rendered as a single button (optionally with a
/// confirm() guard and a danger style).
/// `s` with its first letter in capitals.
fn sentence_case(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A test generation: roll the signups (but those left out) without going
/// live, to find which one breaks generation when its log doesn't say.
fn test_generation_form(subs: &[SubmissionRow]) -> String {
    if subs.is_empty() {
        return String::new();
    }
    let mut boxes = String::new();
    for s in subs {
        let held = if s.held() {
            r#" <span class="statline">(held: left out anyway)</span>"#
        } else {
            ""
        };
        boxes.push_str(&format!(
            r#"<label class="chk"><input type="checkbox" name="exclude" value="{}"> <span>{} <span class="statline">· {} · {}</span>{held}</span></label>"#,
            s.id,
            esc(&s.slot_name),
            esc(&s.game),
            esc(&s.username),
        ));
    }
    format!(
        r#"<details class="testgen"><summary>Test generation</summary>
<p class="statline">Rolls the signups as generating would, without going live: the season stays as it is and the test seed is thrown away. Leave players out to find the one that breaks generation when its log doesn't say.</p>
<form method="post" action="admin/test-generate" class="testgenform"><p class="statline">Leave out:</p>{boxes}<div class="btnrow"><button type="submit" class="ghost">Run a test generation</button></div></form></details>"#
    )
}

/// The last generation's log, in a box of its own that scrolls, with buttons
/// to copy it and to download it. Open when it failed, or was a test.
fn generation_log_box(rec: &crate::generate::GenRecord, log: &str) -> String {
    let what = match rec.kind.as_str() {
        "test" => "Test generation",
        "re-roll" => "Re-roll",
        _ => "Generation",
    };
    let how = if rec.ok { "passed" } else { "failed" };
    let out = if rec.left_out.is_empty() {
        String::new()
    } else {
        format!(" · left out: {}", esc(&rec.left_out.join(", ")))
    };
    let open = if !rec.ok || rec.kind == "test" {
        " open"
    } else {
        ""
    };
    // The page holds the end (where an error is); Download has all of it.
    const INLINE: usize = 200 * 1024;
    let cut = if log.len() > INLINE {
        let mut start = log.len() - INLINE;
        while !log.is_char_boundary(start) {
            start += 1;
        }
        format!(
            "… (the start is cut here: Download has the whole log)\n{}",
            &log[start..]
        )
    } else {
        log.to_string()
    };
    let body = if cut.trim().is_empty() {
        "(it printed nothing)"
    } else {
        cut.as_str()
    };
    format!(
        r#"<details class="genlog"{open}><summary>{what} log · {how} · <span class="ago" data-ts="{}">{}</span>{out}</summary>
<pre id="genlog" tabindex="0">{}</pre>
<div class="btnrow"><button type="button" class="ghost" id="genlogcopy">Copy</button> <a class="pill" href="admin/generation-log" download>Download</a> <span id="genlogmsg" class="statline"></span></div>
<script>(function(){{var b=document.getElementById('genlogcopy'),p=document.getElementById('genlog'),m=document.getElementById('genlogmsg');
// An error is at the end: open the box there (when it's opened later, too).
function end(){{p.scrollTop=p.scrollHeight}}end();p.closest('details').addEventListener('toggle',end);
b.addEventListener('click',function(){{var t=p.textContent;
 function done(){{m.textContent='Copied.';setTimeout(function(){{m.textContent=''}},2000)}}
 function select(){{var r=document.createRange();r.selectNodeContents(p);var s=getSelection();s.removeAllRanges();s.addRange(r);m.textContent='Selected: press Ctrl+C (or ⌘C) to copy.'}}
 if(navigator.clipboard&&window.isSecureContext)navigator.clipboard.writeText(t).then(done,select);else select();}});}})();</script></details>"#,
        rec.at,
        fmt_date(rec.at),
        esc(body),
    )
}

pub(super) fn action_btn(
    action: &str,
    fields: &[(&str, &str)],
    label: &str,
    danger: bool,
    confirm: &str,
) -> String {
    let hidden: String = fields
        .iter()
        .map(|(k, v)| {
            format!(
                r#"<input type="hidden" name="{}" value="{}">"#,
                esc(k),
                esc(v)
            )
        })
        .collect();
    let cls = if danger { "danger" } else { "ghost" };
    let onclick = if confirm.is_empty() {
        String::new()
    } else {
        format!(r#" onclick="return confirm({})""#, js_attr(confirm))
    };
    format!(
        r#"<form method="post" action="{}" style="display:inline">{hidden}<button class="{cls}" type="submit"{onclick}>{}</button></form>"#,
        esc(action),
        esc(label)
    )
}

/// Under the generate button: whether another live season would fit (before
/// it goes live), or what its AP server uses (while live).
pub(super) fn season_load_line(season: &crate::db::SeasonRow) -> String {
    match season.state() {
        crate::db::State::Draft | crate::db::State::Signup => {
            crate::load::room_for_another_season()
                .map(|w| format!(r#"<div class="banner warn">⚠ {}</div>"#, esc(&w)))
                .unwrap_or_default()
        }
        crate::db::State::Live => crate::load::snapshot()
            .and_then(|l| l.latest()?.servers.get(&season.id).copied())
            .map(|(rss, cpu)| {
                format!(
                    r#"<p class="statline">AP server: {} MiB memory, {cpu:.0}% of a CPU core</p>"#,
                    rss >> 20
                )
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// A season's common games, for the admin page.
#[derive(Debug, Default)]
pub struct CommonAdmin {
    pub chosen: Vec<CommonAdminRow>,
    /// Catalog setups it could add: (key, name, slot name).
    pub available: Vec<(String, String, String)>,
    /// Catalog setups that can't be read: (key, why).
    pub broken: Vec<(String, String)>,
    /// Before generation, when the lineup can still change.
    pub lineup_open: bool,
}

#[derive(Debug, Default)]
pub struct CommonAdminRow {
    pub key: String,
    pub name: String,
    pub slot: String,
    /// The season's own address (empty for the default).
    pub address: String,
    pub default_address: String,
    /// The identifier's label and how many names are on the allowlist.
    pub allowlist: Option<(String, usize)>,
    pub problem: Option<String>,
}

/// The Common games section: the season's, with their addresses and
/// allowlists, and the catalog's others to add before generation.
fn common_section(c: &CommonAdmin) -> String {
    let mut b = String::from(r#"<h2>Common games</h2>"#);
    b.push_str(r#"<p class="empty">Co-op games the sysadmin hosts, each played as one slot by everyone in the season. They're set up in the catalog (<code>common/</code> in the data folder) and chosen here before generating. Bifrost doesn't reach their servers: set each one up with its seed file, and keep its allowlist up to date from here.</p>"#);
    if c.chosen.is_empty() {
        b.push_str(r#"<div class="statline">This season has no common games.</div>"#);
    }
    for r in &c.chosen {
        b.push_str(&format!(
            r#"<div class="statline"><b>{}</b> <span class="by free">{}</span> · plays as <b>{}</b></div>"#,
            esc(&r.name),
            esc(&r.key),
            esc(&r.slot)
        ));
        if let Some(p) = &r.problem {
            b.push_str(&format!(r#"<p class="warnnote">{}</p>"#, esc(p)));
        }
        let shown = if r.address.is_empty() {
            &r.default_address
        } else {
            &r.address
        };
        let placeholder = if r.default_address.is_empty() {
            "host:port players join at".to_string()
        } else {
            format!("default: {}", r.default_address)
        };
        b.push_str(&format!(
            r#"<form class="console" method="post" action="admin/common" autocomplete="off"><input type="hidden" name="do" value="address"><input type="hidden" name="key" value="{key}"><input type="text" name="address" value="{addr}" placeholder="{ph}" maxlength="200" style="flex:1;min-width:0" aria-label="Server address"><button type="submit">Set address</button></form>"#,
            key = esc(&r.key),
            addr = esc(&r.address),
            ph = esc(&placeholder),
        ));
        if !shown.is_empty() {
            b.push_str(&format!(
                r#"<p class="empty">Players see <b>{}</b>. Leave it empty for the setup's default.</p>"#,
                esc(shown)
            ));
        }
        let mut btns = String::new();
        if let Some((label, n)) = &r.allowlist {
            b.push_str(&format!(
                r#"<p class="empty">Allowlist: {n} {label}{s} registered by this season's players. Download it <a href="admin/common/{k}/allowlist">as text</a> (one per line) or <a href="admin/common/{k}/allowlist?format=json">as JSON</a>.</p>"#,
                label = esc(label),
                s = if *n == 1 { "" } else { "s" },
                k = esc(&r.key),
            ));
        }
        if c.lineup_open {
            btns.push_str(&action_btn(
                "admin/common",
                &[("do", "remove"), ("key", &r.key)],
                "Remove",
                true,
                "",
            ));
        }
        if !btns.is_empty() {
            b.push_str(&format!(r#"<div class="btnrow">{btns}</div>"#));
        }
    }
    if c.lineup_open {
        if !c.available.is_empty() {
            let opts: String = c
                .available
                .iter()
                .map(|(k, n, slot)| {
                    format!(
                        r#"<option value="{}">{} (plays as {})</option>"#,
                        esc(k),
                        esc(n),
                        esc(slot)
                    )
                })
                .collect();
            b.push_str(&format!(
                r#"<form class="console" method="post" action="admin/common"><input type="hidden" name="do" value="add"><select name="key" style="flex:1;min-width:0" aria-label="Common game">{opts}</select><button type="submit">Add common game</button></form>"#
            ));
        } else if c.broken.is_empty() {
            b.push_str(r#"<p class="empty">The catalog has no other setups.</p>"#);
        }
    } else {
        b.push_str(r#"<p class="empty">The season is generated, so its common games stay as they are. Addresses can still change.</p>"#);
    }
    for (k, e) in &c.broken {
        b.push_str(&format!(
            r#"<p class="warnnote">Catalog setup <code>{}</code> can't be used: {}.</p>"#,
            esc(k),
            esc(e)
        ));
    }
    b
}

/// The admin control centre: generate, open/close signups, manage the deployed
/// players + apworlds, and run server commands. Assumes the caller has
/// already confirmed the viewer is an admin.
#[allow(clippy::too_many_arguments)]
pub fn admin_page(
    season: &crate::db::SeasonRow,
    mods: &SeasonMods,
    connected: bool,
    seed: Option<&str>,
    signups_open: bool,
    minesweeper_on: bool,
    commons: &CommonAdmin,
    connect_pw: &str,
    running: bool,
    gen_msg: &str,
    subs: &[SubmissionRow],
    apworlds: &[crate::generate::Apworld],
    requests: &[crate::db::WorldRequest],
    sysadmin: bool,
    acdata: &str,
    archived: Option<&str>,
    outputs: &[crate::generate::OutputFile],
    out_slots: &[SlotRow],
    connect: Option<&str>,
    managed: bool,
    genlog: Option<&(crate::generate::GenRecord, String)>,
) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="board">← board</a> · season admin · <span class="tag">{}</span> · <a href="/admin/seasons">all seasons</a> · <a href="/admin/users">users &amp; links</a></div>"#,
        esc(&season.name),
        season.state().as_str(),
    ));
    // Tabbed season console; the tab survives form posts via
    // the URL hash (see `TABS_JS`).
    let conn = if connected { "connected" } else { "waiting" };
    let seed_txt = seed.filter(|s| !s.is_empty()).unwrap_or("—");
    let addr = connect
        .map(|a| format!(r#" · players connect to <b class="mono">{}</b>"#, esc(a)))
        .unwrap_or_default();
    b.push_str(&format!(
        r#"<div class="statline">server <b>{conn}</b> · seed <b>{}</b>{addr}</div>"#,
        esc(seed_txt)
    ));
    b.push_str(TABS_CSS);
    b.push_str(r#"<div class="gtabs"><button type="button" class="gtab on" data-tab="tab-overview">Overview</button><button type="button" class="gtab" data-tab="tab-entries">Signups &amp; apworlds</button><button type="button" class="gtab" data-tab="tab-files">Player files</button><button type="button" class="gtab" data-tab="tab-conn">Connection</button><button type="button" class="gtab" data-tab="tab-options">Options</button><button type="button" class="gtab" data-tab="tab-archive">Archive</button></div>"#);

    b.push_str(r#"<div class="gtabpanel" id="tab-overview">"#);
    // Signups
    let state = season.state();
    let pre_gen = matches!(state, crate::db::State::Draft | crate::db::State::Signup);
    // Signups only open or close before generation.
    if pre_gen {
        b.push_str(r#"<h2>Signups</h2>"#);
        if signups_open {
            b.push_str(r#"<div class="statline">Signups are <b>open</b>.</div>"#);
            b.push_str(&format!(
                r#"<div class="btnrow">{}</div>"#,
                action_btn(
                    "admin/signups",
                    &[("open", "0")],
                    "Close signups",
                    false,
                    ""
                )
            ));
        } else {
            b.push_str(
                r#"<div class="statline">Signups are <b>closed</b>: the season is a draft.</div>"#,
            );
            b.push_str(&format!(
                r#"<div class="btnrow">{}</div>"#,
                action_btn("admin/signups", &[("open", "1")], "Open signups", false, "")
            ));
        }
    }

    // Generate, or re-roll once live.
    let n = subs.len();
    let about = match state {
        // While a generation or re-roll runs, the banner says what's happening.
        crate::db::State::Draft | crate::db::State::Signup => format!(
            "Generating rolls a seed from the {n} signup{}, gives each its slot and starts the season's server{}.",
            if n == 1 { "" } else { "s" },
            if signups_open { ", closing signups" } else { "" }
        ),
        crate::db::State::Live => "Regenerating re-rolls the current lineup into a new world with a fresh save, using the season's current host.yaml. Everyone keeps their game; the server restarts.".to_string(),
        _ => String::new(),
    };
    if !about.is_empty() {
        b.push_str(r#"<h2>Generate</h2>"#);
        b.push_str(&format!(r#"<p class="empty">{}</p>"#, esc(&about)));
    }
    let cls = if running {
        "banner"
    } else if gen_msg.starts_with("Failed")
        || gen_msg.starts_with("Test failed")
        || gen_msg.starts_with("Generated, but")
    {
        "banner bad"
    } else {
        "banner ok"
    };
    let mark = if running { "⏳ " } else { "" };
    // Nothing to say before anything has run.
    if running || gen_msg != "idle" {
        b.push_str(&format!(
            r#"<div class="{cls}">{mark}{}</div>"#,
            esc(gen_msg)
        ));
    }
    if running {
        b.push_str(r#"<script>setTimeout(function(){location.reload()},4000)</script>"#);
    } else {
        // A fresh generation for a season that isn't live yet; a re-roll
        // (keeping claims) once it is.
        let btn = match season.state() {
            crate::db::State::Draft | crate::db::State::Signup => action_btn(
                "admin/generate",
                &[],
                "Generate the season",
                false,
                "Generate the season from the signups now? This closes signups and starts the game.",
            ),
            crate::db::State::Live => action_btn(
                "admin/regenerate",
                &[],
                "Regenerate (re-roll current)",
                false,
                "Re-roll the current lineup into a new world with a fresh save? Claims are kept; the game restarts.",
            ),
            _ => String::new(),
        };
        b.push_str(&format!(r#"<div class="btnrow">{btn}</div>"#));
        b.push_str(&season_load_line(season));
        if matches!(
            season.state(),
            crate::db::State::Draft | crate::db::State::Signup
        ) {
            b.push_str(&test_generation_form(subs));
        }
        if let Some((rec, log)) = genlog {
            b.push_str(&generation_log_box(rec, log));
        }
    }

    b.push_str(&mods_section(mods));
    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-entries" hidden>"#);
    // Signups (the queue for the next season)
    let attention: Vec<&SubmissionRow> = subs.iter().filter(|s| s.held_err.is_some()).collect();
    if !attention.is_empty() {
        b.push_str(r#"<h2>Signups that need attention</h2><p class="warnnote">The season can't be generated until each is sorted out: replace the YAML on its page, or dismiss the signup.</p>"#);
        for s in attention {
            b.push_str(&format!(
                r#"<div class="hintrow"><span><a href="signup/game/{}">{}</a> · by {} · <span style="color:var(--bad)">{}</span></span>{}</div>"#,
                s.id,
                esc(&s.slot_name),
                esc(&s.username),
                esc(s.held_err.as_deref().unwrap_or("")),
                action_btn(
                    "admin/delete-player",
                    &[("id", &s.id.to_string())],
                    "dismiss",
                    true,
                    &format!("Dismiss {}'s signup? Its YAML is deleted.", s.slot_name),
                ),
            ));
        }
    }
    b.push_str(r#"<h2>Signups</h2>"#);
    if subs.is_empty() {
        b.push_str(r#"<div class="empty">No signups.</div>"#);
    } else {
        for s in subs {
            let custom = if s.is_custom { " · custom" } else { "" };
            let held = match (&s.held_for, &s.held_err) {
                (Some(_), Some(e)) => format!(
                    r#" · <span style="color:var(--bad)">held: {}</span>"#,
                    esc(e)
                ),
                (Some(w), None) => format!(
                    r#" · <span style="color:var(--warn)">held for {}</span>"#,
                    esc(w)
                ),
                _ => String::new(),
            };
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{} · {}{custom} · by {}{held}</span>{}</div>"#,
                esc(&s.slot_name),
                esc(&s.game),
                esc(&s.username),
                if pre_gen {
                    action_btn(
                        "admin/delete-player",
                        &[("id", &s.id.to_string())],
                        "delete",
                        true,
                        &format!("Delete {}'s signup?", s.slot_name),
                    )
                } else {
                    String::new()
                }
            ));
        }
    }
    if pre_gen {
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "admin/clear",
                &[("what", "signups")],
                "Clear all signups",
                true,
                "Delete every signup and its uploaded files?"
            )
        ));
    }

    // Apworlds waiting for approval
    if !requests.is_empty() {
        b.push_str(r#"<h2>Apworlds waiting for approval</h2><p class="statline">Uploads that aren't in the catalog. Approving installs one and checks the signups held for it; nothing runs its code before then. A sysadmin's approval can add it to the catalog too; a season moderator's passes it to the sysadmins to consider.</p>"#);
        for r in requests {
            let game = if r.game.is_empty() {
                String::new()
            } else {
                format!(" · {}", esc(&r.game))
            };
            let waiting: Vec<String> = subs
                .iter()
                .filter(|s| s.held_for.as_deref() == Some(r.file.as_str()))
                .map(|s| esc(&s.slot_name))
                .collect();
            let game = if waiting.is_empty() {
                game
            } else {
                format!("{game} · signups waiting: {}", waiting.join(", "))
            };
            // A newer version of a game the season has replaces it.
            let dirs = crate::config::get().season(season.id);
            let mut not_newer = false;
            let mut versions = String::new();
            let game = match crate::worlds::installed_for_game(&dirs, &r.game) {
                Some((have, sha, ver)) if sha != r.sha256 => {
                    let have = esc(if ver.is_empty() { &have } else { &ver });
                    let theirs = crate::worlds::manifest_of(&dirs.pending.join(&r.file)).version;
                    if crate::worlds::version_newer(&theirs, &ver) {
                        format!(
                            "{game} · <b>replaces the season's {have}</b>; approving checks every signup for {} against it again",
                            esc(&r.game)
                        )
                    } else {
                        not_newer = true;
                        let v = |v: &str| {
                            if v.is_empty() {
                                "no version".to_string()
                            } else {
                                v.to_string()
                            }
                        };
                        versions = format!(
                            "{} (declares {}) in place of the season's {}",
                            r.file,
                            v(&theirs),
                            v(&ver)
                        );
                        format!(
                            "{game} · the season already has {have}, and this doesn't declare a newer version: reject it, or replace it anyway"
                        )
                    }
                }
                _ => game,
            };
            // Why it couldn't be taken for a new game, so the moderator
            // knows what to look for.
            let game = match crate::worlds::unsure_its_new(&dirs, &r.game) {
                Some(why) => format!(
                    r#"{game} · <span style="color:var(--warn)">{}</span>"#,
                    esc(&sentence_case(&why))
                ),
                None => game,
            };
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{}{game} <span class="statline">· sha256 {}… · uploaded by {} · {}</span></span><span>{} {}</span></div>"#,
                esc(&r.file),
                esc(&r.sha256[..12.min(r.sha256.len())]),
                esc(&r.uploaded_by),
                fmt_date(r.uploaded_at),
                if pre_gen && not_newer {
                    action_btn(
                        "admin/worlds/approve",
                        &[("name", &r.file), ("force", "1")],
                        "replace anyway",
                        false,
                        &format!(
                            "Install {versions}, for {}? It doesn't declare a newer version. Every signup for the game is checked against it again.",
                            r.game
                        ),
                    )
                } else if pre_gen && sysadmin {
                    format!(
                        r#"<form method="post" action="admin/worlds/approve" style="display:inline"><input type="hidden" name="name" value="{}"><label class="inlinechk" title="Vetted for every season: catalog worlds install without approval"><input type="checkbox" name="catalog" value="1" checked> and add to the catalog</label> <button class="ghost" type="submit">approve</button></form>"#,
                        esc(&r.file)
                    )
                } else if pre_gen {
                    action_btn("admin/worlds/approve", &[("name", &r.file)], "approve", false, "")
                } else {
                    String::new()
                },
                action_btn(
                    "admin/worlds/reject",
                    &[("name", &r.file)],
                    "reject",
                    true,
                    &format!("Reject {}? Signups held for it stay held until removed.", r.file)
                ),
            ));
        }
    }

    // Apworlds (installed)
    b.push_str(r#"<h2>Installed apworlds</h2>"#);
    if apworlds.is_empty() {
        b.push_str(r#"<div class="empty">None installed.</div>"#);
    } else {
        for w in apworlds {
            let to_catalog = if sysadmin && w.pinned && !w.catalog {
                action_btn(
                    "admin/worlds/to-catalog",
                    &[("name", &w.file)],
                    "add to catalog",
                    false,
                    "Add this world to the catalog? If the catalog has its game already, this replaces that entry (only with a newer version).",
                )
            } else {
                String::new()
            };
            let update = match &w.update {
                Some(v) if pre_gen => action_btn(
                    "admin/worlds/update",
                    &[("name", &w.file)],
                    &format!("update to the catalog's {v}"),
                    false,
                    &format!(
                        "Replace {} with the catalog's version {v}? Every signup for its game is checked against it again.",
                        w.file
                    ),
                ),
                _ => String::new(),
            };
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{} <span class="statline">· {}</span></span><span>{update} {to_catalog} {}</span></div>"#,
                esc(&w.file),
                esc(&w.detail),
                if pre_gen {
                action_btn(
                    "admin/delete-apworld",
                    &[("name", &w.file)],
                    "delete",
                    true,
                    &format!("Delete {}?", w.file)
                )
                } else {
                    String::new()
                }
            ));
        }
    }
    if pre_gen {
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "admin/clear",
                &[("what", "apworlds")],
                "Clear all apworlds",
                true,
                "Delete every installed custom apworld?"
            )
        ));
    }

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-files" hidden>"#);
    // Generated per-player files (extracted from the current seed) for handing out.
    b.push_str(r#"<h2>Player files</h2>"#);
    if outputs.is_empty() {
        b.push_str(r#"<div class="empty">No per-player files in the current seed. Most games connect directly and produce none; only patch/ROM-based games and a few others (BG3, Minecraft, …) emit one. (Nothing here until a season is generated.)</div>"#);
    } else {
        b.push_str(r#"<p class="empty">Files the generator produced for the seed. A file whose name says which slot it's for is offered on that slot's game page once it's claimed; these copies are for reference or for handing out by hand. A file whose name doesn't say is marked below: no game page offers it, so hand it to the right player yourself.</p>"#);
        for o in outputs {
            let link = format!(
                r#"<a class="ghost" href="admin/output?file={}">download</a>"#,
                urlq(&o.member)
            );
            if o.is_spoiler {
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b>Spoiler</b> · <code>.{}</code> · {} · admin reference, not for players</span>{link}</div>"#,
                    esc(&o.ext),
                    human_size(o.size),
                ));
            } else if o.unassigned {
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b>{}</b> · {} · not matched to a slot (its name doesn't say which), so no game page offers it: hand it to the right player yourself</span>{link}</div>"#,
                    esc(&o.member),
                    human_size(o.size),
                ));
            } else {
                let game = out_slots
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(&o.label))
                    .map(|s| s.game.as_str())
                    .unwrap_or("—");
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b>{}</b>: {} · <code>.{}</code> · {}</span>{link}</div>"#,
                    esc(&o.label),
                    esc(game),
                    esc(&o.ext),
                    human_size(o.size),
                ));
            }
        }
    }

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-conn" hidden>"#);
    // Connect password (what players use to join the server)
    b.push_str(r#"<h2>Connect password</h2>"#);
    b.push_str(&format!(
        r#"<div class="statline">Players join with <b>{}</b>.</div>"#,
        if connect_pw.is_empty() {
            "no password".to_string()
        } else {
            esc(connect_pw)
        }
    ));
    let (confirm, note) = if managed {
        (
            "Change the connect password? It applies to the running server right away.",
            "Letters, numbers, - or _ (3–64 chars). Applies to the running server immediately and to every restart after.",
        )
    } else {
        (
            "Change the connect password? Bifrost will restart briefly to apply it.",
            "Letters, numbers, - or _ (3–64 chars). Sets it on the running server and in host.yaml; Bifrost restarts to reconnect.",
        )
    };
    b.push_str(&format!(
        r#"<form class="console" method="post" action="admin/connect-password" autocomplete="off" onsubmit="return confirm('{confirm}')"><input type="text" name="password" placeholder="new connect password" style="flex:1;min-width:0" autocomplete="off" required><button type="submit">Set</button></form>
<p class="warnnote">{note}</p>"#
    ));

    // AP admin console — commands run over fetch and append to a live log, no
    // page reload (see CONSOLE_JS).
    b.push_str(r#"<h2>Server console</h2>"#);
    b.push_str(r#"<p class="empty">Run a server command as admin, e.g. <code>/send Player Item</code>, <code>/hint Player Item</code>, <code>/players</code>. The <code>!admin</code> prefix is added for you.</p>"#);
    b.push_str(r#"<pre class="out" id="conlog" hidden></pre>"#);
    b.push_str(
        r#"<form class="console" id="conform" method="post" action="admin/console" autocomplete="off"><div class="acwrap"><input type="text" name="cmd" id="concmd" placeholder="/players" autocomplete="off" required><div class="acmenu" id="accands" hidden></div></div><button type="submit">Run</button></form>"#,
    );
    // Command autocomplete: ACDATA (players/games/items/locations) is embedded,
    // then CONSOLE_JS reads it. Split so the JS's braces don't fight format!.
    b.push_str("<script>\n(function(){\nvar ACDATA=");
    b.push_str(&super::script_json(acdata));
    b.push_str(";\n");
    b.push_str(CONSOLE_JS);
    b.push_str("\n})();\n</script>");

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-options" hidden>"#);
    // Minesweeper minigame
    b.push_str(r#"<h2>Minesweeper</h2>"#);
    if minesweeper_on {
        b.push_str(r#"<div class="statline">The minesweeper minigame is <b>enabled</b>, so players can farm bonus hints.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "admin/minesweeper",
                &[("open", "0")],
                "Disable minesweeper",
                false,
                ""
            )
        ));
    } else {
        b.push_str(r#"<div class="statline">The minesweeper minigame is <b>disabled</b>.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "admin/minesweeper",
                &[("open", "1")],
                "Enable minesweeper",
                false,
                ""
            )
        ));
    }

    let cap = season.mine_daily_hints;
    let cap_text = if cap == 0 {
        "no limit".to_string()
    } else {
        format!("at most <b>{cap}</b> in any 24 hours")
    };
    b.push_str(&format!(
        r#"<div class="statline">Bonus hints per player: {cap_text}.</div>
<form class="console" method="post" action="admin/minesweeper/hints" autocomplete="off"><input type="number" name="hints" value="{cap}" min="0" max="{max}" step="1" title="Bonus hints a player can earn in any 24 hours (0 = no limit)" style="flex:1;min-width:0" required><button type="submit">Set limit</button></form>
<p class="empty">Bonus hints a player can earn from minesweeper in any 24 hours; 0 means no limit.</p>"#,
        max = crate::db::MAX_DAILY_HINTS,
    ));

    let limit = season.max_games;
    let limit_text = if limit == 0 {
        "no limit".to_string()
    } else {
        format!("at most <b>{limit}</b>")
    };
    b.push_str(&format!(
        r#"<h2>Games per player</h2>
<div class="statline">Games per player: {limit_text}.</div>
<form class="console" method="post" action="admin/signups/limit" autocomplete="off"><input type="number" name="games" min="0" max="{max}" value="{limit}" style="flex:1;min-width:0"><button type="submit">Set limit</button></form>
<p class="empty">How many games a player can sign up with in this season; 0 means no limit. Signups already made stay.</p>"#,
        max = crate::db::MAX_GAMES_LIMIT,
    ));

    // Who can see the season.
    b.push_str(r#"<h2>Who can see this season</h2>"#);
    b.push_str(&format!(
        r#"<div class="statline">After generation, its pages are for <b>{}</b>.</div><div class="btnrow">{}</div>"#,
        if season.private_live { "its players, moderators and admins" } else { "anyone" },
        if season.private_live {
            action_btn("admin/privacy", &[("what", "live"), ("private", "0")], "Open to anyone", false, "Anyone will be able to see this season's board, log and games. Continue?")
        } else {
            action_btn("admin/privacy", &[("what", "live"), ("private", "1")], "Make private", false, "")
        },
    ));
    let signup_stage = matches!(
        season.state(),
        crate::db::State::Draft | crate::db::State::Signup
    );
    match (&season.private_signup, &season.signup_token) {
        _ if !signup_stage => {}
        (true, Some(t)) => {
            let link = format!("/s/{}/signup/join/{}", season.id, esc(t));
            b.push_str(&format!(
                r#"<div class="statline">Signups are <b>private</b>: only people with this link can reach them.</div>
<p><a class="seatlink" href="{link}">{link}</a></p><div class="btnrow">{}{}</div>
<script>document.querySelectorAll('.seatlink').forEach(function(a){{a.textContent=a.href;}});</script>"#,
                action_btn("admin/privacy", &[("what", "signup"), ("private", "1")], "Make a new link", false, "The current link will stop working. People who already used it keep their access. Continue?"),
                action_btn("admin/privacy", &[("what", "signup"), ("private", "0")], "Open to any user", false, ""),
            ));
        }
        _ => b.push_str(&format!(
            r#"<div class="statline">Signups are open to any signed-in user.</div><div class="btnrow">{}</div>"#,
            action_btn("admin/privacy", &[("what", "signup"), ("private", "1")], "Make private, with a link", false, ""),
        )),
    }

    b.push_str(&common_section(commons));

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-archive" hidden>"#);
    // Archive: live → archived (frozen standings + raw bundle, server stopped).
    if let Some(name) = archived {
        b.push_str(r#"<h2>Archive this season</h2>"#);
        b.push_str(&format!(
            r#"<div class="statline">✓ Archived as <b>{}</b>. <a href="season">View the summary</a></div>"#,
            esc(name),
        ));
    } else if season.state().is_live() {
        b.push_str(r#"<h2>Archive this season</h2>"#);
        b.push_str(r#"<p class="empty">Ends the season: freezes the roster, each game's final standing and the season's minesweeper leaderboards for <b>Past Seasons</b>, writes the raw replayable bundle (multidata, save, YAMLs, apworlds, logs) to the archives folder, then stops the season's AP server (saving) and frees its port.</p>"#);
        b.push_str(&format!(
            r#"<form class="console" method="post" action="admin/archive-season" autocomplete="off" onsubmit="return confirm('Archive this season? Its game server stops.')"><input type="text" name="name" placeholder="{}" title="Final name (optional)" style="flex:1;min-width:0"><button type="submit">Archive &amp; finalize</button></form>"#,
            esc(&season.name)
        ));
    }

    // What can end or undo this season depends on its stage.
    let typed = |action: &str, label: &str, placeholder: &str, confirm: &str| {
        format!(
            r#"<form class="console" method="post" action="{action}" autocomplete="off" onsubmit="return confirm('{confirm}')"><input type="text" name="name" placeholder="{placeholder}" required style="flex:1;min-width:0"><button class="danger" type="submit">{label}</button></form>"#,
            placeholder = esc(placeholder),
        )
    };
    match season.state() {
        crate::db::State::Draft | crate::db::State::Signup => {
            b.push_str(r#"<h2>Delete this season</h2><p class="warnnote">Removes the season and everything in it: signups, YAMLs, notes and apworlds. Type its name to confirm.</p>"#);
            b.push_str(&typed(
                "admin/season/delete",
                "Delete this season",
                &season.name,
                "Delete this season and everything in it? This cannot be undone.",
            ));
        }
        crate::db::State::Live => {
            b.push_str(r#"<h2>Cancel generation</h2><p class="warnnote">Stops the server and goes back to signups, as if the season had never been generated. All game progress is lost, along with its seed, save and logs, and anyone who joined after generation needs a new invite link. Signups, YAMLs, notes, apworlds and partners from before generation stay. Type its name to confirm.</p>"#);
            b.push_str(&typed(
                "admin/cancel",
                "Cancel generation",
                &season.name,
                "Cancel generation? All game progress is lost. This cannot be undone.",
            ));
        }
        crate::db::State::Archived if sysadmin => {
            b.push_str(r#"<h2>Delete this season</h2><p class="warnnote">Removes the finished season, its summary and its bundle. Download the bundle first to keep a copy. Type its name to confirm.</p>"#);
            b.push_str(&typed(
                "admin/season/delete",
                "Delete this season",
                &season.name,
                "Delete this finished season and its bundle? This cannot be undone.",
            ));
        }
        crate::db::State::Generating => {
            b.push_str(r#"<p class="empty">Nothing to do here while it's generating.</p>"#);
        }
        _ => {}
    }

    b.push_str("</div>");

    b.push_str(TABS_JS);
    b.push_str("</main></body></html>");
    Html(b)
}

/// The admin console's client-side command completer. Expects `ACDATA` (from
/// `Board::autocomplete_data`) to be defined just above it. Suggests commands,
/// then player names, then item/location names for the target player's game —
/// the trailing name arg is matched against the whole remainder (names have
/// spaces). Tab accepts the highlighted suggestion; Enter submits.
pub(super) const CONSOLE_JS: &str = r#"var SPEC={
 '/help':{},'/license':{},'/options':{},'/save':{},'/players':{},'/status':{},'/exit':{},'/datastore':{},'/countdown':{},'/option':{},
 '/collect':{f:['player']},'/release':{f:['player']},'/allow_release':{f:['player']},'/forbid_release':{f:['player']},'/alias':{f:['player']},
 '/send':{f:['player'],t:'item'},'/send_multiple':{f:['num','player'],t:'item'},'/hint':{f:['player'],t:'item'},
 '/send_location':{f:['player'],t:'location'},'/hint_location':{f:['player'],t:'location'}
};
var CMDS=Object.keys(SPEC);
var inp=document.getElementById('concmd'),menu=document.getElementById('accands');
var cands=[],sel=-1;
function has(q){q=q.toLowerCase();return function(s){return s.toLowerCase().indexOf(q)>=0}}
function analyze(val){
 if(!/\s/.test(val)){var q=val.toLowerCase();return CMDS.filter(function(c){return c.indexOf(q)===0}).slice(0,12).map(function(c){return {label:c,full:c+' '}});}
 var toks=val.trim().split(/\s+/),cmd=toks[0],spec=SPEC[cmd];
 if(!spec)return [];
 var fixed=spec.f||[],tail=spec.t,args=toks.slice(1),trail=/\s$/.test(val);
 var idx=trail?args.length:args.length-1,cur=trail?'':(args[idx]||'');
 if(idx<fixed.length){
  if(fixed[idx]!=='player')return [];
  return ACDATA.players.filter(has(cur)).slice(0,15).map(function(p){var k=args.slice(0,idx);k.push(p);return {label:p,full:[cmd].concat(k).join(' ')+' '}});
 }
 if(!tail)return [];
 var pIdx=fixed.indexOf('player'),pName=pIdx>=0?args[pIdx]:null,g=pName?ACDATA.game[pName]:null;
 var pool=(tail==='item'?ACDATA.items[g]:ACDATA.locations[g])||[];
 var q=args.slice(fixed.length).join(' ').trim().toLowerCase();
 var lead=[cmd].concat(args.slice(0,fixed.length)).join(' ');
 return pool.filter(function(x){return x.toLowerCase().indexOf(q)>=0}).slice(0,20).map(function(x){return {label:x,full:lead+' '+x}});
}
function render(){
 if(!cands.length){menu.hidden=true;menu.innerHTML='';return;}
 menu.innerHTML='';
 cands.forEach(function(c,i){var d=document.createElement('div');d.className='m'+(i===sel?' sel':'');d.textContent=c.label;d.addEventListener('mousedown',function(e){e.preventDefault();pick(i);});menu.appendChild(d);});
 menu.hidden=false;
 var s=menu.querySelector('.sel');if(s)s.scrollIntoView({block:'nearest'});
}
function refresh(){cands=analyze(inp.value);sel=cands.length?0:-1;render();}
function pick(i){if(i<0||i>=cands.length)return;inp.value=cands[i].full;inp.focus();refresh();}
inp.addEventListener('input',refresh);
inp.addEventListener('focus',refresh);
inp.addEventListener('blur',function(){setTimeout(function(){menu.hidden=true;},150);});
inp.addEventListener('keydown',function(e){
 if(menu.hidden||!cands.length)return;
 if(e.key==='ArrowDown'){e.preventDefault();sel=(sel+1)%cands.length;render();}
 else if(e.key==='ArrowUp'){e.preventDefault();sel=(sel-1+cands.length)%cands.length;render();}
 else if(e.key==='Tab'){e.preventDefault();pick(sel<0?0:sel);}
 else if(e.key==='Escape'){menu.hidden=true;}
});
var form=document.getElementById('conform'),log=document.getElementById('conlog'),run=form.querySelector('button');
function append(t){log.hidden=false;log.textContent+=(log.textContent?'\n':'')+t;log.scrollTop=log.scrollHeight;}
form.addEventListener('submit',function(e){
 e.preventDefault();
 var cmd=inp.value.trim();if(!cmd)return;
 menu.hidden=true;append('$ '+cmd);run.disabled=true;
 fetch('admin/console',{method:'POST',body:new URLSearchParams({cmd:cmd})}).then(function(r){return r.json()}).then(function(j){
  append(j&&j.output?j.output:'(no output)');run.disabled=false;inp.value='';inp.focus();cands=[];render();
 }).catch(function(){append('error: request failed');run.disabled=false;});
});"#;
