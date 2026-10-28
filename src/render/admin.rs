//! The season console (a season's admin page).

use super::*;

/// The "Signed up" roster of submissions, with a Remove control on your own (or,
/// for an admin, any) entry — posts to `/signup/delete`.
pub(super) fn roster(subs: &[SubmissionRow], me: &str, is_admin: bool) -> String {
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
            (Some(_), Some(e)) => format!(
                r#"<div class="meta" style="color:var(--bad)">held: {}</div>"#,
                esc(e)
            ),
            (Some(w), None) => format!(
                r#"<div class="meta">held — waiting for {} to be approved</div>"#,
                esc(w)
            ),
            _ => String::new(),
        };
        let coop = if s.coop_names.is_empty() {
            String::new()
        } else {
            format!(r#"<div class="meta">+ {}</div>"#, esc(&s.coop_names))
        };
        let del = if is_admin || s.username == me {
            format!(
                r#"<form method="post" action="signup/delete" style="display:inline">
<input type="hidden" name="id" value="{}"><button class="ghost" type="submit">Remove</button></form>"#,
                s.id
            )
        } else {
            String::new()
        };
        b.push_str(&format!(
            r#"<div class="slot idle"><div class="game">{}{custom}</div>
<div class="meta">{} · by {}</div>{held}{coop}<div class="seat"><span></span>{del}</div></div>"#,
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

/// The admin control centre: generate, open/close signups, manage the deployed
/// players + apworlds, reset, and run server commands. Assumes the caller has
/// already confirmed the viewer is an admin.
#[allow(clippy::too_many_arguments)]
pub fn admin_page(
    season: &crate::db::SeasonRow,
    mods: &SeasonMods,
    connected: bool,
    seed: Option<&str>,
    signups_open: bool,
    minesweeper_on: bool,
    mc_on: bool,
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
) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="board">← board</a> · season admin · <span class="tag">{}</span> · <a href="/admin/seasons">all seasons</a> · <a href="/admin/users">users &amp; links</a></div>"#,
        esc(&season.name),
        season.state().as_str(),
    ));
    // Tabbed season console; the tab survives form posts via
    // the URL hash (see <button type="button" class="gtab on" data-tab="tab-overview">Overview</button><button type="button" class="gtab" data-tab="tab-entries">Signups &amp; apworlds</button><button type="button" class="gtab" data-tab="tab-files">Player files</button><button type="button" class="gtab" data-tab="tab-conn">Connection</button><button type="button" class="gtab" data-tab="tab-options">Options</button><button type="button" class="gtab" data-tab="tab-archive">Archive &amp; reset</button>_JS).
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
    b.push_str(r#"<div class="gtabs"><button type="button" class="gtab on" data-tab="tab-overview">Overview</button><button type="button" class="gtab" data-tab="tab-entries">Signups &amp; apworlds</button><button type="button" class="gtab" data-tab="tab-files">Player files</button><button type="button" class="gtab" data-tab="tab-conn">Connection</button><button type="button" class="gtab" data-tab="tab-options">Options</button><button type="button" class="gtab" data-tab="tab-archive">Archive &amp; reset</button></div>"#);

    b.push_str(r#"<div class="gtabpanel" id="tab-overview">"#);
    // Signups
    b.push_str(r#"<h2>Signups window</h2>"#);
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
        b.push_str(r#"<div class="statline">Signups are <b>closed</b>.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn("admin/signups", &[("open", "1")], "Open signups", false, "")
        ));
    }

    // Generate
    b.push_str(r#"<h2>Generate</h2>"#);
    b.push_str(&format!(
        r#"<p class="empty"><b>Generate a season</b> stages the {} queued signup(s), rolls a new seed, maps slots + auto-claims, deploys it (the game restarts), and closes signups — use it for a fresh signup round.<br><b>Regenerate</b> re-rolls the <em>current</em> lineup: it reuses the staged YAMLs + installed apworlds, produces a new world + fresh save (applying the current host.yaml), and keeps everyone's claims. Signups are left as they are.</p>"#,
        subs.len()
    ));
    let cls = if running { "banner" } else { "banner ok" };
    let mark = if running { "⏳ " } else { "" };
    b.push_str(&format!(
        r#"<div class="{cls}">{mark}{}</div>"#,
        esc(gen_msg)
    ));
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
    }

    b.push_str(&mods_section(mods));
    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-entries" hidden>"#);
    // Signups (the queue for the next season)
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
                action_btn(
                    "admin/delete-player",
                    &[("id", &s.id.to_string())],
                    "delete",
                    true,
                    &format!("Delete {}'s signup?", s.slot_name)
                )
            ));
        }
    }
    b.push_str(&format!(
        r#"<div class="btnrow">{}</div>"#,
        action_btn(
            "admin/clear",
            &[("what", "signups")],
            "Clear all signups",
            true,
            "Delete every signup and its uploaded files? (The current season's players stay.)"
        )
    ));

    // Apworlds waiting for approval
    if !requests.is_empty() {
        b.push_str(r#"<h2>Apworlds waiting for approval</h2><p class="statline">Uploads that aren't in the catalog. Approving installs one and checks the signups held for it; nothing runs its code before then.</p>"#);
        for r in requests {
            let game = if r.game.is_empty() {
                String::new()
            } else {
                format!(" · {}", esc(&r.game))
            };
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{}{game} <span class="statline">· sha256 {}… · uploaded by {} · {}</span></span><span>{} {}</span></div>"#,
                esc(&r.file),
                esc(&r.sha256[..12.min(r.sha256.len())]),
                esc(&r.uploaded_by),
                fmt_date(r.uploaded_at),
                action_btn("admin/worlds/approve", &[("name", &r.file)], "approve", false, ""),
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
                    "",
                )
            } else {
                String::new()
            };
            b.push_str(&format!(
                r#"<div class="hintrow"><span>{} <span class="statline">· {}</span></span><span>{to_catalog} {}</span></div>"#,
                esc(&w.file),
                esc(&w.detail),
                action_btn(
                    "admin/delete-apworld",
                    &[("name", &w.file)],
                    "delete",
                    true,
                    &format!("Delete {}?", w.file)
                )
            ));
        }
    }
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

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-files" hidden>"#);
    // Generated per-player files (extracted from the current seed) for handing out.
    b.push_str(r#"<h2>Player files</h2>"#);
    if outputs.is_empty() {
        b.push_str(r#"<div class="empty">No per-player files in the current seed — most games connect directly and produce none; only patch/ROM-based games and a few others (BG3, Minecraft, …) emit one. (Nothing here until a season is generated.)</div>"#);
    } else {
        b.push_str(r#"<p class="empty">Files the generator produced for specific slots. Each player already gets theirs on their game page once they've claimed it; these copies are for reference or for handing out by hand. The slot name and game are taken straight from the filename, so there's no guessing which goes where.</p>"#);
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
            } else {
                let game = out_slots
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(&o.label))
                    .map(|s| s.game.as_str())
                    .unwrap_or("—");
                b.push_str(&format!(
                    r#"<div class="hintrow"><span><b>{}</b> — {} · <code>.{}</code> · {}</span>{link}</div>"#,
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
    b.push_str(r#"<p class="empty">Run a server command as admin — e.g. <code>/send Player Item</code>, <code>/hint Player Item</code>, <code>/players</code>. The <code>!admin</code> prefix is added for you.</p>"#);
    b.push_str(r#"<pre class="out" id="conlog" hidden></pre>"#);
    b.push_str(
        r#"<form class="console" id="conform" method="post" action="admin/console" autocomplete="off"><div class="acwrap"><input type="text" name="cmd" id="concmd" placeholder="/players" autocomplete="off" required><div class="acmenu" id="accands" hidden></div></div><button type="submit">Run</button></form>"#,
    );
    // Command autocomplete: ACDATA (players/games/items/locations) is embedded,
    // then CONSOLE_JS reads it. Split so the JS's braces don't fight format!.
    b.push_str("<script>\n(function(){\nvar ACDATA=");
    b.push_str(&acdata.replace("</", "<\\/"));
    b.push_str(";\n");
    b.push_str(CONSOLE_JS);
    b.push_str("\n})();\n</script>");

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-options" hidden>"#);
    // Minesweeper minigame
    b.push_str(r#"<h2>Minesweeper</h2>"#);
    if minesweeper_on {
        b.push_str(r#"<div class="statline">The minesweeper minigame is <b>enabled</b> — players can farm bonus hints.</div>"#);
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

    // Common game (shared Minecraft server) — read at generation time to decide
    // whether the communal in-pool slot is included. Toggle it before generating.
    b.push_str(r#"<h2>Common game (Minecraft)</h2>"#);
    if mc_on {
        b.push_str(r#"<div class="statline">The shared Minecraft common game is <b>enabled</b> — the next season generated will include the communal slot.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "admin/mc",
                &[("open", "0")],
                "Disable common game",
                false,
                ""
            )
        ));
    } else {
        b.push_str(r#"<div class="statline">The shared Minecraft common game is <b>disabled</b> — toggle this <b>before generating</b> to include it in the season.</div>"#);
        b.push_str(&format!(
            r#"<div class="btnrow">{}</div>"#,
            action_btn(
                "admin/mc",
                &[("open", "1")],
                "Enable common game",
                false,
                ""
            )
        ));
    }

    b.push_str("</div>");

    b.push_str(r#"<div class="gtabpanel" id="tab-archive" hidden>"#);
    // Archive: live → archived (frozen standings + raw bundle, server stopped).
    b.push_str(r#"<h2>Archive this season</h2>"#);
    if let Some(name) = archived {
        b.push_str(&format!(
            r#"<div class="statline">✓ Archived as <b>{}</b>. <a href="season">View the summary</a></div>"#,
            esc(name),
        ));
    } else {
        b.push_str(r#"<p class="empty">Ends the season: freezes the roster, each game's final standing and the season's minesweeper leaderboards for <b>Past Seasons</b>, writes the raw replayable bundle (multidata, save, YAMLs, apworlds, logs) to the archives folder, then stops the season's AP server (saving) and frees its port.</p>"#);
        b.push_str(&format!(
            r#"<form class="console" method="post" action="admin/archive-season" autocomplete="off" onsubmit="return confirm('Archive this season? Its game server stops.')"><input type="text" name="name" placeholder="{}" title="Final name (optional)" style="flex:1;min-width:0"><button type="submit">Archive &amp; finalize</button></form>"#,
            esc(&season.name)
        ));
    }

    // Maintenance + reset
    b.push_str(r#"<h2>Maintenance</h2>"#);
    b.push_str(&format!(
        r#"<div class="btnrow">{}{}{}</div>"#,
        action_btn(
            "admin/clear",
            &[("what", "players")],
            "Clear current players",
            true,
            "Delete this season's players — all Bifrost slots, their claims, and the staged YAMLs? (Signups stay.)"
        ),
        action_btn(
            "admin/clear",
            &[("what", "save")],
            "Clear session save",
            true,
            "Delete the running session's save? Progress resets on next server load."
        ),
        action_btn(
            "admin/clear",
            &[("what", "world")],
            "Clear generated world",
            true,
            "Delete the generated world (current.zip + archives)?"
        ),
    ));
    b.push_str(r#"<p class="warnnote">Full reset removes all apworlds, players, slots, the save, and the world, and closes signups — a clean slate for the next season. <b>Archive the season first</b>; past-season snapshots and minesweeper history are kept.</p>"#);
    b.push_str(&format!(
        r#"<div class="btnrow">{}</div>"#,
        action_btn(
            "admin/clear",
            &[("what", "reset")],
            "Full reset",
            true,
            "FULL RESET: wipe apworlds, players, slots, save, and world, and close signups. This cannot be undone. Continue?"
        )
    ));

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
