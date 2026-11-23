//! The season signup wizard.

use super::*;

/// What step 1 of the signup wizard offers: every game the season can play.
pub struct SignupWorlds {
    /// The sandbox can run custom worlds at all.
    pub custom_allowed: bool,
    /// Uploads not in the catalog wait for approval.
    pub manual: bool,
    pub catalog: Vec<crate::db::CatalogWorld>,
    /// The season's Archipelago's core games, once listed.
    pub core: Option<Vec<crate::ap_adapter::GameEntry>>,
    /// Custom games installed in the season: (game, file, version).
    pub installed: Vec<(String, String, String)>,
}

/// One game in the wizard's list.
struct Pick {
    /// "core", "cat" (a catalog world, `value` its hash) or "season".
    kind: &'static str,
    value: String,
    game: String,
    version: String,
}

/// The wizard's one list: games the season has, then the catalog's, then the
/// core games, each game once, sorted by name.
fn picks(worlds: &SignupWorlds) -> Vec<Pick> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    if worlds.custom_allowed {
        // The season's own copy is what its signups use, so its version is
        // the one shown.
        for (game, _, version) in &worlds.installed {
            if seen.insert(game.clone()) {
                out.push(Pick {
                    kind: "season",
                    value: game.clone(),
                    game: game.clone(),
                    version: version.clone(),
                });
            }
        }
        for w in &worlds.catalog {
            let game = if w.game.is_empty() {
                w.file.trim_end_matches(".apworld").to_string()
            } else {
                w.game.clone()
            };
            if seen.insert(game.clone()) {
                out.push(Pick {
                    kind: "cat",
                    value: w.sha256.clone(),
                    game,
                    version: w.version.clone(),
                });
            }
        }
    }
    for g in worlds.core.iter().flatten() {
        if seen.insert(g.game.clone()) {
            out.push(Pick {
                kind: "core",
                value: g.game.clone(),
                game: g.game.clone(),
                version: g.version.clone().unwrap_or_default(),
            });
        }
    }
    out.sort_by_key(|p| p.game.to_lowercase());
    out
}

/// A reason, as a sentence ending in one full stop.
fn sentence(text: &str) -> String {
    format!("{}.", text.trim_end().trim_end_matches('.'))
}

#[allow(clippy::too_many_arguments)]
pub fn signup_page(
    season: &str,
    subs: &[SubmissionRow],
    me: &str,
    is_admin: bool,
    open: bool,
    pre_gen: bool,
    max_games: i64,
    worlds: &SignupWorlds,
) -> Html<String> {
    let mut b = page_head(Some(me));
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="/signups">← Seasons open for signup</a></div>"#,
        esc(season)
    ));
    if !open {
        b.push_str(
            r#"<div class="banner bad">Signups are closed right now. They'll open again before the next season.</div>"#,
        );
        b.push_str(&roster(subs, me, is_admin, pre_gen));
        b.push_str("</main></body></html>");
        return Html(b);
    }
    for s in subs
        .iter()
        .filter(|s| s.username == me && s.held_err.is_some())
    {
        b.push_str(&format!(
            r#"<div class="banner bad">{} needs attention: {} <a href="signup/game/{}">Open its page</a> to replace the YAML.</div>"#,
            esc(&s.slot_name),
            esc(&sentence(s.held_err.as_deref().unwrap_or(""))),
            s.id
        ));
    }
    // Someone who has a game isn't signing up again: they may add another, on
    // purpose, so the form waits behind a button.
    let mine = subs.iter().filter(|s| s.username == me).count() as i64;
    if max_games > 0 && mine >= max_games {
        b.push_str(&format!(
            r#"<div class="banner ok">You've reached this season's limit of {max_games} game(s) per player. Your signups are below.</div>"#
        ));
        b.push_str(&roster(subs, me, is_admin, pre_gen));
        b.push_str("</main></body></html>");
        return Html(b);
    }
    let limit_note = if max_games > 0 {
        format!(" This season allows {max_games} game(s) per player.")
    } else {
        String::new()
    };
    let (wizard_open, wizard_close) = if mine > 0 {
        (
            format!(
                r#"<p class="empty">You're signed up with {mine} game(s), listed below.{limit_note}</p>
<div class="btnrow"><button type="button" id="addgame">Add another game</button></div>
<div id="wizbox" hidden>"#
            ),
            r#"</div>
<script>document.getElementById('addgame').addEventListener('click',function(){document.getElementById('wizbox').hidden=false;this.hidden=true;});</script>"#,
        )
    } else if max_games > 0 {
        (
            format!(r#"<p class="empty">{}</p>"#, limit_note.trim_start()),
            "",
        )
    } else {
        (String::new(), "")
    };
    b.push_str(&wizard_open);
    // Step 1: one searchable list of every game the season can play, with
    // uploading another custom world as its last entry.
    let mut items = String::new();
    if worlds.core.is_none() {
        items.push_str(r#"<li role="option" tabindex="0" data-kind="core" data-value="" data-game=""><span>A core game <small>(the full list is still being gathered; your YAML names the game)</small></span></li>"#);
    }
    for p in picks(worlds) {
        let ver = if p.version.is_empty() {
            String::new()
        } else {
            format!(r#"<span class="v">{}</span>"#, esc(&p.version))
        };
        let tag = if p.kind == "core" {
            ""
        } else {
            r#" <span class="tag">custom</span>"#
        };
        items.push_str(&format!(
            r#"<li role="option" tabindex="0" data-kind="{}" data-value="{}" data-game="{}"><span>{}{tag}</span>{ver}</li>"#,
            p.kind,
            esc(&p.value),
            esc(&p.game),
            esc(&p.game),
        ));
    }
    if worlds.custom_allowed {
        items.push_str(r#"<li role="option" tabindex="0" data-kind="upload" data-value="" data-game="" class="upload"><span>Upload another custom world (<code>.apworld</code>)…</span></li>"#);
    }
    let mut step1 = format!(
        r#"<input type="search" id="gamefind" class="gamefind" placeholder="Search for your game" aria-label="Search for your game" aria-controls="gamelist" autocomplete="off">
      <ul id="gamelist" class="gamepick" role="listbox" aria-label="Games">{items}</ul>
      <p class="empty" id="nogame" hidden>No game matches. Check the spelling, or upload its <code>.apworld</code> if it's a custom game.</p>
      <div id="pickmsg" class="inlinemsg"></div>"#
    );
    if worlds.custom_allowed {
        step1.push_str(r#"<p class="hint">Each game shows the version the server has. If you have a newer version of a custom game, upload it. Once it's approved it replaces the season's version, and signups for that game are checked against it again.</p>"#);
    }
    if !worlds.custom_allowed {
        step1.push_str(
            r#"<p class="hint">Custom (unofficial) games are turned off on this server.</p>"#,
        );
    } else {
        let approval = if worlds.manual {
            " If it isn't in the catalog, a moderator has to approve it before it can run, and your signup is held until then."
        } else {
            " It installs on the server right away so we can validate your YAML next."
        };
        step1.push_str(&format!(
            r#"<div id="uploadbox" hidden>
      <p class="hint">Upload the world's <code>.apworld</code>.{approval}</p>
      <div class="pickrow"><input type="file" id="apfile" accept=".apworld"><button type="button" id="apbtn" class="back">Upload apworld</button></div>
      <div id="apmsg" class="inlinemsg"></div>
    </div>"#
        ));
    }
    b.push_str(
        &r#"<p class="empty">Two quick steps. We check everything against the server as you go, so the seed rolls clean when the season is generated.</p>
<ol class="stepper">
  <li data-step="1"><span class="n">1</span> Game</li>
  <li data-step="2"><span class="n">2</span> YAML</li>
</ol>
<form id="wiz" method="post" action="signup" enctype="multipart/form-data">
  <section class="wstep" data-step="1">
    <h2><!--GAMEHEAD--></h2>
<!--STEP1-->
    <div class="nav"><button type="button" id="tostep2" data-goto="2">Next →</button></div>
  </section>
  <section class="wstep" data-step="2" hidden>
    <h2>Options YAML</h2>
    <div id="tplrow" hidden><p class="hint">New to this game's options? Start from a template with every option, its default and what it does.</p>
    <div class="pickrow"><button type="button" id="tpllink" class="back">Download a starting YAML for <span id="tplgame"></span></button></div><div id="tplmsg" class="inlinemsg"></div></div>
    <p class="hint" id="yamlhint">Upload your options file. It's rolled against the installed worlds and has to pass before you can sign up.</p>
    <div class="pickrow"><input type="file" name="yaml" id="yamlfile" accept=".yaml,.yml" required><button type="button" id="checkbtn" class="back">Check it</button></div>
    <div id="checkmsg" class="inlinemsg"></div>
    <div class="nav"><button type="button" class="back" data-goto="1">← Back</button><button type="submit" id="wizsubmit" disabled><!--SUBMIT--></button></div>
    <input type="hidden" name="is_custom" id="iscustom" value="">
    <input type="hidden" name="pending_world" id="pendingworld" value="">
  </section>
</form>
<script>
(function(){
 var form=document.getElementById('wiz');
 function show(n){
   form.querySelectorAll('.wstep').forEach(function(s){s.hidden=s.dataset.step!=String(n)});
   document.querySelectorAll('.stepper li').forEach(function(li){var s=Number(li.dataset.step);li.classList.toggle('active',s==n);li.classList.toggle('done',s<n)});
 }
 form.querySelectorAll('[data-goto]').forEach(function(btn){btn.addEventListener('click',function(){if(!btn.disabled)show(Number(btn.dataset.goto))})});
 var state={ready:false,pending:'',game:'',custom:false},next1=document.getElementById('tostep2');
 var chkbtn=document.getElementById('checkbtn'),chkmsg=document.getElementById('checkmsg'),next2=document.getElementById('wizsubmit'),yamlfile=document.getElementById('yamlfile');
 var find=document.getElementById('gamefind'),list=document.getElementById('gamelist'),nogame=document.getElementById('nogame'),pickmsg=document.getElementById('pickmsg'),upbox=document.getElementById('uploadbox');
 var items=[].slice.call(list.querySelectorAll('li'));
 function refresh1(){
   document.getElementById('iscustom').value=state.custom?'1':'';
   document.getElementById('pendingworld').value=state.pending;
   chkbtn.hidden=!!state.pending;
   document.getElementById('yamlhint').textContent=state.pending?('Upload your options file. It can\'t be checked until '+state.pending+' is approved, so your signup is held and checked then.'):'Upload your options file. It\'s rolled against the installed worlds and has to pass before you continue.';
   next2.disabled=!(state.pending&&yamlfile.files.length);chkmsg.textContent='';
   next1.disabled=!state.ready;
   var tr=document.getElementById('tplrow');tr.hidden=!(state.game&&!state.pending);
   document.getElementById('tplgame').textContent=state.game;document.getElementById('tplmsg').textContent='';
 }
 // Search: narrow the list as you type (the upload entry always stays).
 find.addEventListener('input',function(){var q=find.value.trim().toLowerCase(),n=0;
   items.forEach(function(li){var ok=li.dataset.kind=='upload'||!q||li.textContent.toLowerCase().indexOf(q)>=0;li.hidden=!ok;if(ok&&li.dataset.kind!='upload')n++;});
   nogame.hidden=n>0;});
 function choose(li){
   items.forEach(function(x){x.setAttribute('aria-selected',x===li?'true':'false');});
   var k=li.dataset.kind;upbox&&(upbox.hidden=k!='upload');pickmsg.className='inlinemsg';pickmsg.textContent='';
   state={ready:false,pending:'',game:li.dataset.game,custom:k!='core'};
   if(k=='core'||k=='season'){state.ready=true;}
   else if(k=='cat'){var fd=new FormData();fd.append('catalog',li.dataset.value);place(pickmsg,fd);}
   else if(k=='upload'){state.game='';}
   refresh1();
 }
 items.forEach(function(li){li.addEventListener('click',function(){choose(li)});
   li.addEventListener('keydown',function(e){if(e.key=='Enter'||e.key==' '){e.preventDefault();choose(li);}});});
 function place(msg,fd,btn){
   msg.className='inlinemsg';msg.textContent='Installing…';if(btn)btn.disabled=true;
   fetch('signup/apworld',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
     if(btn)btn.disabled=false;
     if(j.ok){state.ready=true;state.pending=j.pending?j.world:'';if(j.game)state.game=j.game;state.custom=!j.core;
       msg.className='inlinemsg ok';msg.textContent=j.note||(j.pending?('Uploaded '+j.world+'. It needs a moderator\'s approval before it can run. You can continue; your signup is held until then.'):('Installed '+j.world+'. You can continue.'));}
     else{state.ready=false;msg.className='inlinemsg bad';msg.textContent=j.error||'Install failed.';}
     refresh1();
   }).catch(function(){if(btn)btn.disabled=false;msg.className='inlinemsg bad';msg.textContent='Install failed (network).';});
 }
 var apbtn=document.getElementById('apbtn');
 if(apbtn)apbtn.addEventListener('click',function(){
   var f=document.getElementById('apfile').files[0],m=document.getElementById('apmsg');
   if(!f){m.className='inlinemsg bad';m.textContent='Choose an .apworld file first.';return;}
   var fd=new FormData();fd.append('apworld',f);place(m,fd,apbtn);
 });
 // The starting YAML: written on first ask, so ask again until it's ready.
 document.getElementById('tpllink').addEventListener('click',function(e){
   e.preventDefault();var game=state.game,msg=document.getElementById('tplmsg'),tries=0;
   msg.className='inlinemsg';msg.textContent='Preparing…';
   (function get(){fetch('signup/template?game='+encodeURIComponent(game)).then(function(r){
     if(r.status==202){if(++tries>60){msg.className='inlinemsg bad';msg.textContent='It\'s taking too long. Try again later.';return;}setTimeout(get,3000);return;}
     if(!r.ok)return r.json().then(function(j){msg.className='inlinemsg bad';msg.textContent=j.error||'No template for this game.';});
     return r.blob().then(function(b){var a=document.createElement('a');a.href=URL.createObjectURL(b);a.download=game+'.yaml';document.body.appendChild(a);a.click();a.remove();msg.textContent='';});
   }).catch(function(){msg.className='inlinemsg bad';msg.textContent='Download failed (network).';});})();
 });
 refresh1();
 yamlfile.addEventListener('change',function(){next2.disabled=!(state.pending&&yamlfile.files.length);chkmsg.textContent='';});
 chkbtn.addEventListener('click',function(){
   var f=yamlfile.files[0];
   if(!f){chkmsg.className='inlinemsg bad';chkmsg.textContent='Choose your YAML first.';return;}
   chkmsg.className='inlinemsg';chkmsg.textContent='Checking…';chkbtn.disabled=true;
   var fd=new FormData();fd.append('yaml',f);
   fetch('signup/check',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
     chkbtn.disabled=false;
     if(j.ok){chkmsg.className='inlinemsg ok';chkmsg.textContent='✓ '+(j.name||'slot')+' ('+(j.game||'?')+') rolled cleanly.';next2.disabled=false;}
     else{next2.disabled=true;chkmsg.className='inlinemsg bad';chkmsg.textContent=(j.errors&&j.errors.length?j.errors.join('; '):(j.error||'Did not pass.'));}
   }).catch(function(){chkbtn.disabled=false;chkmsg.className='inlinemsg bad';chkmsg.textContent='Check failed (network).';});
 });
 show(1);
})();
</script>"#
            .replace("<!--STEP1-->", &step1)
            .replace(
                "<!--GAMEHEAD-->",
                if mine > 0 { "Your other game" } else { "Your game" },
            )
            .replace(
                "<!--SUBMIT-->",
                if mine > 0 { "Add this game" } else { "Sign me up" },
            ),
    );
    b.push_str(wizard_close);

    b.push_str(&roster(subs, me, is_admin, pre_gen));
    b.push_str("</main></body></html>");
    Html(b)
}

/// The seasons open for signup, as cards leading to each season's page.
pub fn signups_list(seasons: &[(Id, String, Vec<SubmissionRow>)]) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(
        r#"<h1>Sign up for a season</h1><div class="crumb"><a href="/">← Bifrost</a></div>"#,
    );
    if seasons.is_empty() {
        b.push_str(r#"<div class="empty">No season is open for signup right now.</div>"#);
    } else {
        b.push_str(r#"<div class="grid">"#);
        for (id, name, subs) in seasons {
            let players = subs
                .iter()
                .map(|s| s.account_id)
                .collect::<std::collections::HashSet<_>>()
                .len();
            b.push_str(&format!(
                r#"<div class="slot idle stretch"><div class="game"><a class="cover" href="/s/{id}/signup">{}</a></div>
<div class="meta">{} game(s), {players} player(s)</div></div>"#,
                esc(name),
                subs.len(),
            ));
        }
        b.push_str("</div>");
    }
    b.push_str("</main></body></html>");
    Html(b)
}

/// What a game's page shows its viewer.
pub struct SignupGame<'a> {
    pub sub: &'a SubmissionRow,
    pub seats: &'a [crate::db::SeatRow],
    /// The viewer's own seat, while it can still be renamed.
    pub my_seat: Option<&'a crate::db::SeatRow>,
    pub is_host: bool,
    pub pre_gen: bool,
    pub edit_notes: bool,
    /// Why it's held is for those playing it and those running the season.
    pub show_reason: bool,
    pub replace: bool,
    /// Shown just after signing up.
    pub welcome: Option<String>,
    /// What was just changed.
    pub saved: Option<String>,
    /// The invite box, for the host.
    pub invite: String,
    pub max_notes: usize,
    /// The season is live, so the game has its own page to point to.
    pub live: bool,
}

/// The box a game's host invites co-op partners from: a one-use link each.
pub fn invite_box(action: &str, link_base: &str, tokens: &[String]) -> String {
    let mut b = String::from(
        r#"<h2>Co-op partners</h2><p class="empty">Each link seats one person with an account here. They pick their own seat name.</p>"#,
    );
    for t in tokens {
        b.push_str(&format!(
            r#"<div class="seat"><a class="seatlink" href="{link_base}{t}">{link_base}{t}</a><form method="post" action="{action}/revoke" style="display:inline"><input type="hidden" name="token" value="{t}"><button class="ghost" type="submit">Revoke</button></form></div>"#,
            link_base = esc(link_base),
            t = esc(t),
            action = esc(action),
        ));
    }
    b.push_str(&format!(
        r#"<form method="post" action="{}/invite"><button type="submit">Create an invite link</button></form>
<script>document.querySelectorAll('.seatlink').forEach(function(a){{a.textContent=a.href;}});</script>"#,
        esc(action)
    ));
    b
}

/// A seat link's page: who's inviting you, and your seat's name.
pub fn seat_page(host: &str, game: &str, season: &str, token: &str) -> Html<String> {
    let mut b = page_head(None);
    b.push_str(&format!(
        r#"<h1>Join {game}</h1><p>{host} has invited you to join them for <b>{game}</b> in {season}.</p>
<form class="signup" method="post" action="seat/{token}" autocomplete="off"><label>Your seat's name<input type="text" id="seatname" name="name" maxlength="16" required></label><button type="submit" id="seatgo" disabled>Claim this seat</button></form>
<script>(function(){{var n=document.getElementById('seatname'),g=document.getElementById('seatgo');n.addEventListener('input',function(){{g.disabled=!n.value.trim();}});}})();</script>
</main></body></html>"#,
        host = esc(host),
        game = esc(game),
        season = esc(season),
        token = esc(token),
    ));
    Html(b)
}

/// A game before the season generates. Anyone signed in can read it; the forms
/// are for its host, partners and the season's moderators.
pub fn signup_game_page(g: &SignupGame) -> Html<String> {
    let s = g.sub;
    let (edit_notes, replace, max_notes) = (g.edit_notes, g.replace, g.max_notes);
    let mut b = page_head(None);
    b.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="signup">← season</a></div>
<div class="meta">{} · by {}</div>"#,
        esc(&s.slot_name),
        esc(&s.game),
        esc(&s.username),
    ));
    if let Some(s) = &g.saved {
        b.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(s)));
    }
    if let Some(w) = &g.welcome {
        b.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(w)));
    }
    if g.live {
        b.push_str(&format!(
            r#"<p class="statline">The season is live: hints, trackers and files are on <a href="game/{}">the game's page</a>.</p>"#,
            s.id
        ));
    }
    if !g.pre_gen && !s.seat_names.is_empty() {
        b.push_str(&format!(
            r#"<div class="meta">+ {}</div>"#,
            esc(&s.seat_names)
        ));
    }
    for seat in g.seats.iter().filter(|_| g.pre_gen) {
        let remove = if g.is_host && g.pre_gen {
            format!(
                r#"<form method="post" action="signup/game/{}/seat/remove" style="display:inline"><input type="hidden" name="seat" value="{}"><button class="ghost" type="submit">Remove</button></form>"#,
                s.id, seat.id
            )
        } else {
            String::new()
        };
        b.push_str(&format!(
            r#"<div class="seat"><span class="who">{}</span><span><span class="by">{}</span> {remove}</span></div>"#,
            esc(&seat.name),
            esc(&seat.username),
        ));
    }
    if let Some(seat) = g.my_seat {
        b.push_str(&format!(
            r#"<form class="signup" method="post" action="signup/game/{}/seat/rename"><input type="hidden" name="seat" value="{}"><label>Your seat's name<input type="text" name="name" maxlength="16" value="{}" required></label><button type="submit">Rename</button></form>
<form method="post" action="signup/game/{}/seat/leave" onsubmit="return confirm('Leave this game?')"><button class="ghost" type="submit">Leave this game</button></form>"#,
            s.id,
            seat.id,
            esc(&seat.name),
            s.id,
        ));
    }
    b.push_str(&g.invite);
    match (&s.held_for, &s.held_err) {
        // Just signed up: the welcome has said this already.
        (Some(_), None) if g.welcome.is_some() => {}
        (Some(_), Some(_)) if !g.show_reason => {
            b.push_str(r#"<div class="banner bad">This game is held.</div>"#)
        }
        (Some(_), Some(e)) => b.push_str(&format!(
            r#"<div class="banner bad">This game needs attention: {} Until it's sorted out, or removed, the season can't be generated.</div>"#,
            esc(&sentence(e))
        )),
        (Some(w), None) => b.push_str(&format!(
            r#"<div class="banner ok">Held, waiting for {} to be approved. Its YAML is checked then. You can replace it meanwhile, and the new file gets checked instead.</div>"#,
            esc(w)
        )),
        _ => {}
    }
    b.push_str("<h2>Notes</h2>");
    if edit_notes {
        b.push_str(&format!(
            r#"<p class="empty">For the season's other players, shown on this game's card.</p>
<form class="signup" method="post" action="signup/game/{}/notes"><textarea name="notes" rows="5" maxlength="{max_notes}">{}</textarea><button type="submit">Save notes</button></form>"#,
            s.id,
            esc(&s.notes),
        ));
    } else if s.notes.is_empty() {
        b.push_str(r#"<div class="empty">No notes.</div>"#);
    } else {
        b.push_str(&format!(r#"<div class="notes">{}</div>"#, esc(&s.notes)));
    }
    if replace {
        b.push_str(&format!(
            r#"<h2>Replace the YAML</h2>
<p class="empty">The new file is checked like a new signup. To rename the slot, change <code>name:</code> in it. A custom game's world has to be installed already, unless this one is still waiting for approval.</p>
<form class="signup" method="post" action="signup/game/{}/yaml" enctype="multipart/form-data"><input type="file" name="yaml" accept=".yaml,.yml" required><button type="submit">Replace</button></form>"#,
            s.id
        ));
    }
    b.push_str("</main></body></html>");
    Html(b)
}

pub fn signup_result(text: &str, ok: bool) -> Html<String> {
    let mut b = page_head(None);
    let cls = if ok { "banner ok" } else { "banner bad" };
    b.push_str(&format!(
        r#"<h1>Signup</h1><div class="crumb"><a href="signup">← signup</a></div>
<div class="{cls}">{}</div>"#,
        esc(text)
    ));
    b.push_str("</main></body></html>");
    Html(b)
}
