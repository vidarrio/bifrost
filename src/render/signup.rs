//! The season signup wizard.

use super::*;

/// What step 1 of the signup wizard offers for custom games.
pub struct SignupWorlds {
    /// The sandbox can run custom worlds at all.
    pub custom_allowed: bool,
    /// Uploads not in the catalog wait for approval.
    pub manual: bool,
    pub catalog: Vec<crate::db::CatalogWorld>,
}

/// A catalog entry as one line: "Game 1.2 (file)".
pub(super) fn catalog_label(w: &crate::db::CatalogWorld) -> String {
    let name = if w.game.is_empty() {
        w.file.trim_end_matches(".apworld")
    } else {
        &w.game
    };
    let mut l = name.to_string();
    if !w.version.is_empty() {
        l.push_str(&format!(" {}", w.version));
    }
    l.push_str(&format!(" ({})", w.file));
    l
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
            r#"<div class="banner bad">{} needs attention: {}. <a href="signup/game/{}">Open its page</a> to replace the YAML.</div>"#,
            esc(&s.slot_name),
            esc(s.held_err.as_deref().unwrap_or("")),
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
    let mut step1 = String::from(
        r#"<div class="choice">
      <label><input type="radio" name="gtype" value="core" checked> <span>A core / officially supported game</span></label>"#,
    );
    if worlds.custom_allowed && !worlds.catalog.is_empty() {
        step1.push_str(r#"<label><input type="radio" name="gtype" value="catalog"> <span>A custom game from this server's catalog</span></label>"#);
    }
    if worlds.custom_allowed {
        step1.push_str(r#"<label><input type="radio" name="gtype" value="custom"> <span>Another custom (unofficial) game — I have its <code>.apworld</code></span></label>"#);
    }
    step1.push_str("</div>");
    if !worlds.custom_allowed {
        step1.push_str(
            r#"<p class="hint">Custom (unofficial) games are turned off on this server.</p>"#,
        );
    }
    if worlds.custom_allowed && !worlds.catalog.is_empty() {
        let opts: String = worlds
            .catalog
            .iter()
            .map(|w| {
                format!(
                    r#"<option value="{}">{}</option>"#,
                    esc(&w.sha256),
                    esc(&catalog_label(w))
                )
            })
            .collect();
        step1.push_str(&format!(
            r#"<div data-box="catalog" hidden>
      <p class="hint">Worlds the server's admins have vetted — they're ready to use.</p>
      <select id="catsel">{opts}</select>
      <button type="button" id="catbtn" class="back">Use this world</button>
      <div id="catmsg" class="inlinemsg"></div>
    </div>"#
        ));
    }
    if worlds.custom_allowed {
        let approval = if worlds.manual {
            " If it isn't in the catalog, a moderator has to approve it before it can run — your signup is held until then."
        } else {
            " It installs on the server right away so we can validate your YAML next."
        };
        step1.push_str(&format!(
            r#"<div data-box="custom" hidden>
      <p class="hint">Upload the world's <code>.apworld</code>.{approval}</p>
      <input type="file" id="apfile" accept=".apworld">
      <button type="button" id="apbtn" class="back">Upload apworld</button>
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
    <p class="hint" id="yamlhint">Upload your options file. It's rolled against the installed worlds — it has to pass before you can sign up.</p>
    <input type="file" name="yaml" id="yamlfile" accept=".yaml,.yml" required>
    <button type="button" id="checkbtn" class="back">Check it</button>
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
 var ready={core:true,catalog:false,custom:false},pending='',next1=document.getElementById('tostep2');
 var chkbtn=document.getElementById('checkbtn'),chkmsg=document.getElementById('checkmsg'),next2=document.getElementById('wizsubmit'),yamlfile=document.getElementById('yamlfile');
 function gtype(){var r=form.querySelector('input[name=gtype]:checked');return r?r.value:'core';}
 function refresh1(){
   var t=gtype();
   form.querySelectorAll('[data-box]').forEach(function(b){b.hidden=b.dataset.box!=t;});
   document.getElementById('iscustom').value=t=='core'?'':'1';
   var p=t=='custom'?pending:'';
   document.getElementById('pendingworld').value=p;
   chkbtn.hidden=!!p;
   document.getElementById('yamlhint').textContent=p?('Upload your options file. It can\'t be checked until '+p+' is approved — your signup is held and checked then.'):'Upload your options file. It\'s rolled against the installed worlds — it has to pass before you continue.';
   next2.disabled=!(p&&yamlfile.files.length);chkmsg.textContent='';
   next1.disabled=!ready[t];
 }
 form.querySelectorAll('input[name=gtype]').forEach(function(r){r.addEventListener('change',refresh1)});
 function place(btn,msg,fd,kind){
   msg.className='inlinemsg';msg.textContent='Installing…';btn.disabled=true;
   fetch('signup/apworld',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
     btn.disabled=false;
     if(j.ok&&j.pending){ready[kind]=true;pending=j.world;msg.className='inlinemsg ok';msg.textContent='Uploaded '+j.world+' — it needs a moderator\'s approval before it can run. You can continue; your signup is held until then.';}
     else if(j.ok){ready[kind]=true;if(kind=='custom')pending='';msg.className='inlinemsg ok';msg.textContent='Installed '+j.world+'. You can continue.';}
     else{ready[kind]=false;msg.className='inlinemsg bad';msg.textContent=j.error||'Install failed.';}
     refresh1();
   }).catch(function(){btn.disabled=false;msg.className='inlinemsg bad';msg.textContent='Install failed (network).';});
 }
 var apbtn=document.getElementById('apbtn');
 if(apbtn)apbtn.addEventListener('click',function(){
   var f=document.getElementById('apfile').files[0],m=document.getElementById('apmsg');
   if(!f){m.className='inlinemsg bad';m.textContent='Choose an .apworld file first.';return;}
   var fd=new FormData();fd.append('apworld',f);place(apbtn,m,fd,'custom');
 });
 var catbtn=document.getElementById('catbtn');
 if(catbtn)catbtn.addEventListener('click',function(){
   var fd=new FormData();fd.append('catalog',document.getElementById('catsel').value);
   place(catbtn,document.getElementById('catmsg'),fd,'catalog');
 });
 var catsel=document.getElementById('catsel');
 if(catsel)catsel.addEventListener('change',function(){ready.catalog=false;document.getElementById('catmsg').textContent='';refresh1();});
 refresh1();
 yamlfile.addEventListener('change',function(){next2.disabled=!(pending&&gtype()=='custom'&&yamlfile.files.length);chkmsg.textContent='';});
 chkbtn.addEventListener('click',function(){
   var f=yamlfile.files[0];
   if(!f){chkmsg.className='inlinemsg bad';chkmsg.textContent='Choose your YAML first.';return;}
   chkmsg.className='inlinemsg';chkmsg.textContent='Checking…';chkbtn.disabled=true;
   var fd=new FormData();fd.append('yaml',f);
   fetch('signup/check',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(j){
     chkbtn.disabled=false;
     if(j.ok){chkmsg.className='inlinemsg ok';chkmsg.textContent='✓ '+(j.name||'slot')+' — '+(j.game||'?')+' rolled cleanly.';next2.disabled=false;}
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
    pub replace: bool,
    /// Shown just after signing up.
    pub welcome: Option<String>,
    /// The invite box, for the host.
    pub invite: String,
    pub max_notes: usize,
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
    if let Some(w) = &g.welcome {
        b.push_str(&format!(r#"<div class="banner ok">{}</div>"#, esc(w)));
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
        (Some(_), Some(e)) => b.push_str(&format!(
            r#"<div class="banner bad">This game needs attention: {}. Until it's sorted out, or removed, the season can't be generated.</div>"#,
            esc(e)
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
