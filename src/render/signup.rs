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

pub fn signup_page(
    subs: &[SubmissionRow],
    me: &str,
    is_admin: bool,
    open: bool,
    worlds: &SignupWorlds,
) -> Html<String> {
    let mut b = page_head(Some(me));
    b.push_str(r#"<h1>Season signup</h1><div class="crumb"><a href="/">← Bifrost</a></div>"#);
    if !open {
        b.push_str(
            r#"<div class="banner bad">Signups are closed right now. They'll open again before the next season.</div>"#,
        );
        b.push_str(&roster(subs, me, is_admin));
        b.push_str("</main></body></html>");
        return Html(b);
    }
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
        &r#"<p class="empty">Three quick steps. We check everything against the server as you go, so the seed rolls clean when the season is generated.</p>
<ol class="stepper">
  <li data-step="1"><span class="n">1</span> Game</li>
  <li data-step="2"><span class="n">2</span> YAML</li>
  <li data-step="3"><span class="n">3</span> Players</li>
</ol>
<form id="wiz" method="post" action="signup" enctype="multipart/form-data">
  <section class="wstep" data-step="1">
    <h2>Your game</h2>
<!--STEP1-->
    <div class="nav"><button type="button" id="tostep2" data-goto="2">Next →</button></div>
  </section>
  <section class="wstep" data-step="2" hidden>
    <h2>Options YAML</h2>
    <p class="hint" id="yamlhint">Upload your options file. It's rolled against the installed worlds — it has to pass before you continue.</p>
    <input type="file" name="yaml" id="yamlfile" accept=".yaml,.yml" required>
    <button type="button" id="checkbtn" class="back">Check it</button>
    <div id="checkmsg" class="inlinemsg"></div>
    <div class="nav"><button type="button" class="back" data-goto="1">← Back</button><button type="button" id="tostep3" data-goto="3" disabled>Next →</button></div>
  </section>
  <section class="wstep" data-step="3" hidden>
    <h2>Players</h2>
    <div class="coopown">You: <b id="myname">your slot</b> <span class="by free">from your YAML — already in</span></div>
    <label class="chk"><input type="checkbox" id="coopchk"> Sharing this game between people (co-op, e.g. Don't Starve Together)</label>
    <div id="coopbox" hidden>
      <label class="hint">You're already included above — just list the <b>other</b> people sharing this game, comma-separated. They can claim their seat once the season is generated.<input type="text" name="coop" placeholder="e.g. Robin, Sam"></label>
    </div>
    <input type="hidden" name="is_custom" id="iscustom" value="">
    <input type="hidden" name="pending_world" id="pendingworld" value="">
    <div class="nav"><button type="button" class="back" data-goto="2">← Back</button><button type="submit">Sign me up</button></div>
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
 var chkbtn=document.getElementById('checkbtn'),chkmsg=document.getElementById('checkmsg'),next2=document.getElementById('tostep3'),yamlfile=document.getElementById('yamlfile');
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
     if(j.ok){chkmsg.className='inlinemsg ok';chkmsg.textContent='✓ '+(j.name||'slot')+' — '+(j.game||'?')+' rolled cleanly.';var mn=document.getElementById('myname');if(mn&&j.name)mn.textContent=j.name;next2.disabled=false;}
     else{next2.disabled=true;chkmsg.className='inlinemsg bad';chkmsg.textContent=(j.errors&&j.errors.length?j.errors.join('; '):(j.error||'Did not pass.'));}
   }).catch(function(){chkbtn.disabled=false;chkmsg.className='inlinemsg bad';chkmsg.textContent='Check failed (network).';});
 });
 var coopchk=document.getElementById('coopchk');
 coopchk.addEventListener('change',function(){document.getElementById('coopbox').hidden=!coopchk.checked;});
 show(1);
})();
</script>"#
            .replace("<!--STEP1-->", &step1),
    );

    b.push_str(&roster(subs, me, is_admin));
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
