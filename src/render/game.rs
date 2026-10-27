//! A slot's game page: hints, Universal Tracker, PopTracker, log, files.

use super::*;

/// The per-game hint search. Expects `ITEMS` (this world's item names) defined
/// just above it. Substring filter with a live dropdown; ↑/↓ highlight (and
/// scroll into view), Tab accepts, Enter submits the form.
pub(super) const HINT_JS: &str = r#"var q=document.getElementById('hintq'),box=document.getElementById('hintmatches');
var hits=[],hsel=-1;
function hrender(){
 box.innerHTML='';
 hits.forEach(function(n,i){var d=document.createElement('div');d.className='match'+(i===hsel?' sel':'');d.textContent=n;d.addEventListener('mousedown',function(e){e.preventDefault();q.value=n;hits=[];box.innerHTML='';q.focus();});box.appendChild(d);});
 var s=box.querySelector('.sel');if(s)s.scrollIntoView({block:'nearest'});
}
q.addEventListener('input',function(){
 var v=q.value.toLowerCase().trim();if(!v){hits=[];box.innerHTML='';return;}
 var w=v.split(/\s+/);
 hits=ITEMS.filter(function(n){var l=n.toLowerCase();return w.every(function(x){return l.indexOf(x)>=0})}).slice(0,15);
 hsel=hits.length?0:-1;hrender();
});
q.addEventListener('keydown',function(e){
 if(!hits.length)return;
 if(e.key==='ArrowDown'){e.preventDefault();hsel=(hsel+1)%hits.length;hrender();}
 else if(e.key==='ArrowUp'){e.preventDefault();hsel=(hsel-1+hits.length)%hits.length;hrender();}
 else if(e.key==='Tab'){e.preventDefault();if(hsel>=0){q.value=hits[hsel];hits=[];box.innerHTML='';}}
 else if(e.key==='Escape'){hits=[];box.innerHTML='';}
});
var hform=document.getElementById('hintform'),hstatus=document.getElementById('hintstatus');
var hback=hform.querySelector('input[name=back]').value,hbtn=hform.querySelector('button');
function refreshHints(){
 var el=document.getElementById('worldhints');
 [0,1200,2800].forEach(function(d){setTimeout(function(){
  if(el)fetch(hback.slice(1)+'/hints').then(function(r){return r.text()}).then(function(h){el.innerHTML=h;}).catch(function(){});
  // Refresh the search pool too: a name whose last copy was just hinted drops out.
  fetch(hback.slice(1)+'/items').then(function(r){return r.json()}).then(function(a){if(Array.isArray(a))ITEMS=a;}).catch(function(){});
 },d);});
}
hform.addEventListener('submit',function(e){
 e.preventDefault();
 var item=q.value.trim();if(!item)return;
 hits=[];box.innerHTML='';hbtn.disabled=true;hstatus.className='inlinemsg';hstatus.textContent='Hinting…';
 fetch('hint',{method:'POST',body:new URLSearchParams(new FormData(hform))}).then(function(r){return r.json()}).then(function(j){
  hbtn.disabled=false;
  hstatus.className='inlinemsg '+(j&&j.ok?'ok':'bad');
  hstatus.textContent=(j&&j.message)?j.message:(j&&j.ok?'Hinted.':'Hint failed.');
  if(j&&j.ok){q.value='';refreshHints();var pe=document.getElementById('hintpts');if(pe&&j.points!=null)pe.textContent=j.points;var ce=document.getElementById('bonushints');if(ce&&j.credits!=null)ce.textContent=j.credits;}
 }).catch(function(){hbtn.disabled=false;hstatus.className='inlinemsg bad';hstatus.textContent='Request failed.';});
});"#;

/// Client requirements for the shared Minecraft common game — these MUST match
/// what's installed on the Minecraft LXC (120). Update all three together when
/// the common game's versions change.
pub(super) const MC_GAME_VERSION: &str = "26.2"; // Minecraft Java Edition version
pub(super) const MC_LOADER: &str = "NeoForge 26.2.0.90"; // mod loader build
// The AP mod jar name is `crate::generate::MC_MOD_JAR` (shared with the download
// route that serves it), so the displayed name and the served file can't drift.

/// The per-game page: claim/unclaim its players, hint with live item search,
/// and all hints in this world with a "prioritize" toggle.
#[allow(clippy::too_many_arguments)]
/// Game-page tab styling (Hints | Universal Tracker). Distinctive class names:
/// the shared stylesheet uses short utility names, so a generic class would
/// silently inherit unrelated rules (see the header note in this file).
pub(super) const GAME_STYLE: &str = r#"<style>
#tab-log>h2{text-align:center}
#tab-log .feed,#tab-log .pager{margin-inline:auto}
.gamelogbar{margin:0 0 10px;text-align:center}
.gamelogbar input[type=search]{width:100%;max-width:420px;box-sizing:border-box;background:var(--bg);color:var(--fg);border:1px solid var(--bd);border-radius:7px;padding:8px 11px;font-size:13px;font-family:var(--font)}
.gamelogbar input[type=search]:focus{outline:none;border-color:var(--accent)}
.utmeta{font-family:var(--mono);font-size:12px;color:var(--muted);margin:0 0 12px}
.utlist{display:flex;flex-direction:column;gap:4px;max-height:70vh;overflow:auto}
.utloc{font-size:13px;line-height:1.35;padding:6px 10px;background:var(--panel);border:1px solid var(--bd);border-radius:7px}
.utnote{font-size:12px;color:var(--muted);margin-top:14px;font-style:italic}
#tab-pt>h2{text-align:center}
.ptmap{width:100vw;max-width:100vw;margin-left:calc(50% - 50vw);margin-right:calc(50% - 50vw);padding-inline:clamp(16px,3vw,48px);box-sizing:border-box;text-align:center}
.ptmapwrap{position:relative;display:inline-block;max-width:100%;vertical-align:top;margin:0 7px 16px}
.ptgroup .ptgh{text-align:center}
/* Zoom/pan viewport: clips the transformed stage; touch-action:none so a drag
   pans instead of scrolling the page. */
.ptview{position:relative;overflow:hidden;width:fit-content;max-width:100%;border-radius:4px;touch-action:none}
.ptstage{position:relative;transform-origin:0 0}
.ptview img,.ptmapwrap img{display:block;max-width:100%;height:auto;user-select:none;-webkit-user-drag:none}
.ptmk{position:absolute;border:1px solid rgba(0,0,0,.65);box-sizing:border-box;border-radius:2px}
.ptmk.d{transform:rotate(45deg)}
.ptform{margin:10px 0;display:flex;gap:8px;align-items:center;flex-wrap:wrap}
.ptform input[type=file]{color:var(--muted);font-size:12.5px;max-width:100%}
.ptform input[type=file]::file-selector-button{background:transparent;border:1px solid var(--bd);color:var(--muted);border-radius:6px;padding:5px 11px;margin-right:10px;font-family:var(--font);font-size:12px;font-weight:600;cursor:pointer}
.ptform input[type=file]::file-selector-button:hover{border-color:var(--accent);color:var(--accent)}
.ptmanage{margin-top:16px;text-align:center}.ptmanage>summary{cursor:pointer;color:var(--muted);font-size:13px}
.ptmanage .ptform{justify-content:center}
.ptnest{margin:4px 0}
.pttabs{display:inline-flex;flex-wrap:wrap;gap:2px;justify-content:center;border-bottom:1px solid var(--bd);margin:0 0 10px}
.pttab{background:none;color:var(--muted);border:0;border-bottom:2px solid transparent;border-radius:0;padding:5px 11px;font-family:var(--font);font-size:13px;font-weight:600;cursor:pointer;margin-bottom:-1px}
.pttab:hover{color:var(--fg)}.pttab.on{color:var(--accent);border-bottom-color:var(--accent)}
.ptgroup{margin:6px 0}.ptgh{font-family:var(--mono);font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);margin:10px 0 4px}
.ptdock{display:flex;flex-direction:column;gap:14px}
.ptdockmid{display:flex;flex-direction:row;flex-wrap:wrap;gap:16px;justify-content:center;align-items:flex-start}
/* A bottom-docked strip (e.g. the item panels) gets a divider from the map
   above it. Shrink the strip to its content and center it so the divider line
   spans the item strip, not the full map width. */
.ptdockmid+.ptdock{border-top:1px solid var(--bd);padding-top:12px;align-self:center;width:fit-content;max-width:100%}
.ptarr{display:flex;gap:10px;flex-wrap:wrap;justify-content:center}
.ptgrid{display:inline-block;margin:4px 0}
.ptgrow{display:flex;gap:3px;justify-content:flex-start}
.ptcell{position:relative;display:inline-flex;width:34px;height:34px;align-items:center;justify-content:center}
.ptcell.empty{visibility:hidden}
.ptitem{width:100%;height:100%;object-fit:contain;image-rendering:pixelated}
.ptitem.ptoff{opacity:.25;filter:grayscale(.7)}
.ptov{position:absolute;right:1px;bottom:0;font-family:var(--mono);font-size:10px;font-weight:700;color:#fff;text-shadow:0 0 2px #000,0 0 2px #000,0 0 2px #000;pointer-events:none}
</style>"#;

/// The game page's log tab (after [`TABS_JS`]).
pub(super) const GAME_TAB_JS: &str = r#"<script>
// The game's Log tab: a type-to-filter search + a feed whose page size is
// fitted to the viewport, with live refresh of the current page while open.
// Pager links (carrying the active search + rpp) are intercepted to swap in
// place. The first page is server-rendered; JS refits on show/resize.
(function(){var host=document.getElementById('gamelog');if(!host)return;
var slot=host.getAttribute('data-slot'),panel=document.getElementById('tab-log'),
    search=document.getElementById('gamelogq'),cur=null,tmr=null,rt=null,rpp=25;
function into(u){cur=u;fetch(u).then(function(r){return r.text();}).then(function(x){host.innerHTML=x;if(window.bifTick)bifTick();}).catch(function(){});}
function fit(){ // how many rows fit below the feed's top, within sane bounds
 var top=host.getBoundingClientRect().top,row=30,ev=host.querySelector('.ev');
 if(ev){var h=ev.getBoundingClientRect().height;if(h>0)row=h+6;}
 var n=Math.floor((window.innerHeight-top-64)/row);
 if(n<8)n=8;if(n>200)n=200;return n;}
function reload(page){rpp=fit();var q=search?search.value.trim():'';
 into('game/'+slot+'/log/fragment?page='+(page||1)+'&rpp='+rpp+(q?('&q='+encodeURIComponent(q)):''));}
host.addEventListener('click',function(e){var a=e.target.closest('a.pg');if(!a)return;e.preventDefault();into(a.getAttribute('href'));});
if(search){search.addEventListener('input',function(){clearTimeout(tmr);tmr=setTimeout(function(){reload(1);},200);});}
var bar=document.querySelector('.gtabs');
if(bar){bar.addEventListener('click',function(e){var b=e.target.closest('.gtab');if(b&&b.getAttribute('data-tab')==='tab-log')reload(1);});}
window.addEventListener('resize',function(){if(panel&&panel.hidden)return;clearTimeout(rt);rt=setTimeout(function(){reload(1);},250);});
setInterval(function(){if(panel&&!panel.hidden&&cur)into(cur);},12000);
})();
</script>"#;

/// Script that fills the PopTracker tab: fetch the computed tracker state and
/// draw each map image with accessibility-coloured markers at their pixel
/// coordinates (PopTracker palette: green available, yellow glitch, blue
/// viewable, red blocked, grey collected; mixed locations split diagonally),
/// polling while the tab is open.
pub(super) const PT_JS: &str = r#"<script>
(function(){
 var host=document.getElementById('ptmap'); if(!host) return;
 var slot=host.getAttribute('data-slot'); var st={}; var ver='';
 // PopTracker's exact default marker colors, keyed by accessibility level.
 var PT_COL={reachable:'#20ff20',glitch:'#ffff20',inspect:'#3040ff',none:'#cf1010',done:'#3f3f3f',partial:'#ff9f20'};
 // Rank so a split shows the best state top-left, worst bottom-right.
 var PT_RANK={reachable:4,glitch:3,inspect:2,none:1};
 function secLevel(s){ return s.checked?'done':(s.level||(s.accessible?'reachable':'none')); }
 function secColor(s){ return PT_COL[secLevel(s)]||PT_COL.none; }
 function secWord(s){ var l=secLevel(s);
  return l==='done'?'collected':l==='reachable'?'available':l==='glitch'?'available (glitch)':l==='inspect'?'viewable only':'blocked'; }
 // Background for a marker. Collected checks drop out (as in PopTracker); the
 // color reflects the checks that remain:
 //   all collected        -> grey
 //   one remaining state   -> that solid color
 //   two remaining states  -> diagonal split, bottom-left to top-right
 //   three or more states  -> solid orange "partial"
 function markerBg(k){
  var secs=(k.sections||[]);
  if(!secs.length){ return k.checked?PT_COL.done:(k.accessible?PT_COL.reachable:PT_COL.none); }
  var rem=secs.filter(function(s){return !s.checked;});
  if(!rem.length) return PT_COL.done;
  var cols=rem.map(secColor).filter(function(c,i,a){return a.indexOf(c)===i;});
  if(cols.length===1) return cols[0];
  if(cols.length>=3) return PT_COL.partial;
  cols.sort(function(a,b){ return (ptRankOf(b)-ptRankOf(a)); });
  // 135deg puts the dividing line bottom-left -> top-right; first color top-left.
  return 'linear-gradient(135deg,'+cols[0]+' 50%,'+cols[1]+' 50%)';
 }
 function ptRankOf(hex){ for(var k in PT_COL){ if(PT_COL[k]===hex&&PT_RANK[k]) return PT_RANK[k]; } return 0; }
 // Tooltip: the location name, then each check's name with its cleared/total
 // count and state (PopTracker shows every section a location holds).
 function markerTitle(k){
  var secs=(k.sections||[]);
  if(secs.length<=1){
   var s0=secs[0];
   var word=s0?secWord(s0):(k.checked?'collected':(k.accessible?'available':'blocked'));
   if(s0&&s0.item_count>1){ return k.location+' ('+word+')\n'+s0.cleared+'/'+s0.item_count+' checks'; }
   return k.location+' ('+word+')';
  }
  var lines=[k.location];
  secs.forEach(function(s){ lines.push('• '+(s.name||'(section)')+': '+s.cleared+'/'+s.item_count+' ('+secWord(s)+')'); });
  return lines.join('\n');
 }
 function assetUrl(f){ return 'game/'+slot+'/poptracker/asset?file='+encodeURIComponent(f)+(ver?('&v='+ver):''); }
 function drawMap(mapName){
  var m=(st.maps||[]).filter(function(x){return x.name===mapName;})[0];
  var wrap=document.createElement('div'); wrap.className='ptmapwrap';
  if(!m){ wrap.textContent='(map '+mapName+' missing)'; return wrap; }
  // A clipping viewport (.ptview) holds a transformable stage (.ptstage) with
  // the image + markers; scroll zooms toward the cursor, drag pans (like native
  // PopTracker). Markers are % of the stage, so they ride the transform.
  var view=document.createElement('div'); view.className='ptview';
  var stage=document.createElement('div'); stage.className='ptstage';
  view.appendChild(stage); wrap.appendChild(view);
  var img=document.createElement('img'); img.loading='lazy'; img.draggable=false;
  img.src=assetUrl(m.img); stage.appendChild(img);
  var sz=m.location_size||16; var spans=[];
  (st.markers||[]).filter(function(k){return k.map===mapName;}).forEach(function(k){
   var s=k.size||sz; var mk=document.createElement('span');
   mk.className='ptmk'+(((k.shape||m.location_shape)==='diamond')?' d':'');
   mk.dataset.key=k.location+'@'+k.map; mk.dataset.loc=k.location;
   mk.title=markerTitle(k);
   mk.style.background=markerBg(k);
   mk._k=k; mk._s=s; spans.push(mk); stage.appendChild(mk);
  });
  // Position markers as PERCENTAGES of the image's natural size, so they scale
  // with the responsive (max-width:100%) image on any screen width.
  function place(){ var W=img.naturalWidth||1, H=img.naturalHeight||1;
   spans.forEach(function(mk){ var k=mk._k, s=mk._s;
    mk.style.left=((k.x-s/2)/W*100)+'%'; mk.style.top=((k.y-s/2)/H*100)+'%';
    mk.style.width=(s/W*100)+'%'; mk.style.height=(s/H*100)+'%';
   });
  }
  if(img.complete && img.naturalWidth) place(); else img.addEventListener('load',place);
  attachZoomPan(view,stage);
  return wrap;
 }
 // Scroll-to-zoom + drag-to-pan (mouse) and pinch/drag (touch) on one map view.
 function attachZoomPan(view,stage){
  var scale=1,tx=0,ty=0,MAX=8;
  function apply(){ stage.style.transform='translate('+tx+'px,'+ty+'px) scale('+scale+')';
   view.style.cursor=scale>1?'grab':''; view.classList.toggle('zoomed',scale>1); }
  function clamp(){ var r=view.getBoundingClientRect(), W=r.width, H=r.height;
   var minx=W*(1-scale), miny=H*(1-scale);
   tx=Math.min(0,Math.max(minx,tx)); ty=Math.min(0,Math.max(miny,ty));
   if(scale<=1){ tx=0; ty=0; } }
  function zoomAt(px,py,factor){ var ns=Math.min(MAX,Math.max(1,scale*factor));
   if(ns===scale) return; var cx=(px-tx)/scale, cy=(py-ty)/scale;
   scale=ns; tx=px-cx*scale; ty=py-cy*scale; clamp(); apply(); }
  view.addEventListener('wheel',function(e){ e.preventDefault(); var r=view.getBoundingClientRect();
   zoomAt(e.clientX-r.left, e.clientY-r.top, e.deltaY<0?1.2:1/1.2); }, {passive:false});
  view.addEventListener('dblclick',function(e){ e.preventDefault(); scale=1; tx=0; ty=0; apply(); });
  var pts={}, last=null, pinch=0;
  view.addEventListener('pointerdown',function(e){ pts[e.pointerId]={x:e.clientX,y:e.clientY};
   try{ view.setPointerCapture(e.pointerId); }catch(_){}
   if(Object.keys(pts).length===1){ last={x:e.clientX,y:e.clientY}; if(scale>1) view.style.cursor='grabbing'; } });
  view.addEventListener('pointermove',function(e){ if(!pts[e.pointerId]) return;
   pts[e.pointerId]={x:e.clientX,y:e.clientY}; var ids=Object.keys(pts);
   if(ids.length>=2){ var a=pts[ids[0]], b=pts[ids[1]], d=Math.hypot(a.x-b.x,a.y-b.y);
    var r=view.getBoundingClientRect(); if(pinch) zoomAt((a.x+b.x)/2-r.left,(a.y+b.y)/2-r.top,d/pinch);
    pinch=d; last=null; return; }
   if(last){ if(scale>1){ tx+=e.clientX-last.x; ty+=e.clientY-last.y; clamp(); apply(); }
    last={x:e.clientX,y:e.clientY}; } });
  function up(e){ delete pts[e.pointerId]; try{ view.releasePointerCapture(e.pointerId); }catch(_){}
   if(Object.keys(pts).length<2) pinch=0;
   if(!Object.keys(pts).length){ last=null; view.style.cursor=scale>1?'grab':''; } }
  view.addEventListener('pointerup',up); view.addEventListener('pointercancel',up);
  apply();
 }
 var itemsByCode={};
 function buildIndex(){ itemsByCode={}; (st.items||[]).forEach(function(it){ (it.codes||[]).forEach(function(c){ if(c) itemsByCode[(''+c).toLowerCase()]=it; }); }); }
 function itemCell(code){
  var cell=document.createElement('span'); cell.className='ptcell';
  if(!code){ cell.classList.add('empty'); return cell; }
  var it=itemsByCode[(''+code).toLowerCase()];
  if(it&&it.img){
   var im=document.createElement('img'); im.className='ptitem'+(it.active?'':' ptoff'); im.dataset.code=code; im.src=assetUrl(it.img); im.alt=it.name||code;
   cell.title=it.name||code; cell.appendChild(im);
   if(it.overlay){ var o=document.createElement('span'); o.className='ptov'; o.textContent=it.overlay; cell.appendChild(o); }
  }
  return cell;
 }
 function appendContent(parent,c){ if(c==null) return; if(Array.isArray(c)){ c.forEach(function(x){ parent.appendChild(render(x)); }); } else { parent.appendChild(render(c)); } }
 function render(n){
  if(n==null) return document.createComment('');
  if(Array.isArray(n)){ var f=document.createElement('div'); n.forEach(function(c){f.appendChild(render(c));}); return f; }
  if(typeof n!=='object') return document.createComment('');
  var t=n.type;
  if(t==='map'){ var d=document.createElement('div'); (n.maps||[]).forEach(function(mn){d.appendChild(drawMap(mn));}); return d; }
  if(t==='tabbed'){
   var box=document.createElement('div'); box.className='ptnest';
   var bar=document.createElement('div'); bar.className='pttabs';
   var body=document.createElement('div'); box.appendChild(bar); box.appendChild(body);
   (n.tabs||[]).forEach(function(tab,i){
    var btn=document.createElement('button'); btn.type='button'; btn.className='pttab'+(i===0?' on':''); btn.textContent=tab.title||('Tab '+(i+1));
    var panel=render(tab.content); panel.hidden=(i!==0);
    btn.addEventListener('click',function(){
     bar.querySelectorAll('.pttab').forEach(function(x){x.classList.remove('on');}); btn.classList.add('on');
     Array.prototype.forEach.call(body.children,function(p){p.hidden=true;}); panel.hidden=false;
    });
    bar.appendChild(btn); body.appendChild(panel);
   });
   return box;
  }
  if(t==='itemgrid'){ var g=document.createElement('div'); g.className='ptgrid';
   (n.rows||[]).forEach(function(row){ var r=document.createElement('div'); r.className='ptgrow'; (row||[]).forEach(function(code){ r.appendChild(itemCell(code)); }); g.appendChild(r); });
   return g;
  }
  if(t==='item'){ var ic=document.createElement('div'); ic.className='ptgrow'; ic.appendChild(itemCell(n.item||n.code||'')); return ic; }
  if(t==='array'){ var a=document.createElement('div'); a.className='ptarr';
   a.style.flexDirection=(n.orientation==='horizontal'?'row':'column'); a.style.alignItems=(n.orientation==='horizontal'?'flex-start':'center');
   appendContent(a,n.content); return a;
  }
  if(t==='group'){ var gr=document.createElement('div'); gr.className='ptgroup';
   if(n.header){ var h=document.createElement('div'); h.className='ptgh'; h.textContent=n.header; gr.appendChild(h); }
   appendContent(gr,n.content); return gr;
  }
  // PopTracker dock: children carry a `dock` edge (top/bottom/left/right) and
  // undocked children fill the center. We honor it so docked panels land on
  // their edge — e.g. Stardew's item grids are dock:"bottom", so they belong
  // under the map, not above it (array order alone would float them to the top).
  if(t==='dock'){
   var kids=[]; (function col(c){ if(c==null) return; if(Array.isArray(c)) c.forEach(col); else kids.push(c); })(n.content);
   var b={top:[],bottom:[],left:[],right:[],center:[]};
   kids.forEach(function(k){ (b[(k&&k.dock)]||b.center).push(k); });
   var box=document.createElement('div'); box.className='ptdock';
   b.top.forEach(function(k){ box.appendChild(render(k)); });
   if(b.left.length||b.center.length||b.right.length){
    var mid=document.createElement('div'); mid.className='ptdockmid';
    b.left.concat(b.center,b.right).forEach(function(k){ mid.appendChild(render(k)); });
    box.appendChild(mid);
   }
   b.bottom.forEach(function(k){ box.appendChild(render(k)); });
   return box;
  }
  if(n.content!==undefined){ var cc=document.createElement('div'); appendContent(cc,n.content); return cc; }
  return document.createComment(t||'');
 }
 function update(){
  var mm={}; (st.markers||[]).forEach(function(k){mm[k.location+'@'+k.map]=k;});
  host.querySelectorAll('.ptmk').forEach(function(mk){ var k=mm[mk.dataset.key]; if(k){ mk.style.background=markerBg(k); mk.title=markerTitle(k); } });
  host.querySelectorAll('.ptitem').forEach(function(im){ var it=itemsByCode[im.dataset.code]; if(!it) return;
   im.className='ptitem'+(it.active?'':' ptoff'); if(it.img) im.src=assetUrl(it.img);
   var ov=im.parentNode.querySelector('.ptov');
   if(it.overlay){ if(!ov){ ov=document.createElement('span'); ov.className='ptov'; im.parentNode.appendChild(ov); } ov.textContent=it.overlay; }
   else if(ov){ ov.remove(); }
  });
 }
 function draw(d){
  if(!d||!d.pack){ host.textContent='No pack.'; return; }
  if(d.error){ host.textContent='Tracker error: '+d.error; return; }
  ver=(d.version!=null?(''+d.version):''); st=d.state||{}; buildIndex();
  if(host.dataset.built){ update(); return; }
  host.innerHTML='';
  if(st.layout){ host.appendChild(render(st.layout)); }
  else if(st.maps&&st.maps.length){ st.maps.forEach(function(m){ host.appendChild(drawMap(m.name)); }); }
  else { host.textContent='This pack has no maps to show.'; return; }
  host.dataset.built='1';
 }
 function load(){ fetch('game/'+slot+'/poptracker/state').then(function(r){return r.json();}).then(draw).catch(function(){}); }
 var panel=document.getElementById('tab-pt'); var bar=document.querySelector('.gtabs');
 if(bar){ bar.addEventListener('click',function(e){var b=e.target.closest('.gtab'); if(b&&b.getAttribute('data-tab')==='tab-pt') load();}); }
 load();
 setInterval(function(){ if(panel&&!panel.hidden) load(); },12000);
})();
</script>"#;

/// The PopTracker tab panel: the map host (filled by `PT_JS`) + upload/manage
/// controls for a claimant or admin, or a prompt/placeholder otherwise.
pub(super) fn poptracker_panel(ap_slot: i64, has_pack: bool, can_manage: bool) -> String {
    let mut s = String::from(r#"<div class="gtabpanel" id="tab-pt" hidden><h2>PopTracker</h2>"#);
    if has_pack {
        s.push_str(&format!(
            r#"<div class="ptmap" id="ptmap" data-slot="{ap_slot}"><p class="empty">Loading tracker…</p></div>"#
        ));
        if can_manage {
            s.push_str(&format!(
                r#"<details class="ptmanage"><summary>Manage pack</summary>
<form method="post" action="game/{ap_slot}/poptracker" enctype="multipart/form-data" class="ptform">
<input type="file" name="pack" accept=".zip,application/zip" required><button type="submit">Replace pack</button></form>
<form method="post" action="game/{ap_slot}/poptracker/remove" onsubmit="return confirm('Remove this pack?')"><button type="submit" class="ghost">Remove pack</button></form></details>"#
            ));
        }
        s.push_str(PT_JS);
    } else if can_manage {
        s.push_str(&format!(
            r#"<p class="empty">Upload a <a href="https://github.com/black-sliver/PopTracker" target="_blank" rel="noopener">PopTracker</a> pack (.zip) for this game to get its map here, auto-tracked from the server — no setup for anyone viewing. The pack must support Archipelago auto-tracking.</p>
<form method="post" action="game/{ap_slot}/poptracker" enctype="multipart/form-data" class="ptform">
<input type="file" name="pack" accept=".zip,application/zip" required><button type="submit">Upload pack</button></form>"#
        ));
    } else {
        s.push_str(r#"<p class="empty">No PopTracker pack has been uploaded for this game yet.</p>"#);
    }
    s.push_str("</div>");
    s
}

/// Game Log tab page size: the server default (before the client fits it to the
/// viewport) and the clamp bounds the fitted value must stay within.
pub const GAME_LOG_RPP: i64 = 25;
pub const GAME_LOG_RPP_MIN: i64 = 8;
pub const GAME_LOG_RPP_MAX: i64 = 200;

/// Pages for a given total at `rpp` (at least 1).
pub fn game_log_pages(total: i64, rpp: i64) -> i64 {
    ((total + rpp - 1) / rpp.max(1)).max(1)
}

/// Inner HTML of a game's Log feed (the `#gamelog` contents): one page of rows
/// plus a numbered pager. Reused by the tab and its live-refresh fragment; the
/// pager links carry the active search `qs` + `rpp` and are intercepted
/// client-side so navigation swaps in place.
pub fn game_log_feed(
    ap_slot: i64,
    rows: &[crate::db::ActivityRow],
    page: i64,
    pages: i64,
    rpp: i64,
    qs: &str,
) -> String {
    let mut s = String::new();
    if rows.is_empty() {
        s.push_str(r#"<p class="empty">Nothing matches.</p>"#);
        return s;
    }
    render_feed(&mut s, rows, None);
    let base = format!("game/{ap_slot}/log/fragment");
    s.push_str(&activity_pager(&base, qs, page, pages, rpp));
    s
}

/// The game page's "Log" tab: this slot's own activity (items it sent or
/// received, its goals/releases/collects), newest first, with a type-to-filter
/// search box and a viewport-fitted paginated feed.
pub(super) fn log_panel(ap_slot: i64, rows: &[crate::db::ActivityRow], total: i64) -> String {
    let mut s = String::from(
        r#"<div class="gtabpanel" id="tab-log" hidden><h2>This game's activity</h2>
<div class="gamelogbar"><input type="search" id="gamelogq" placeholder="Search this game's activity…" autocomplete="off"></div>"#,
    );
    s.push_str(&format!(r#"<div id="gamelog" class="gamelog" data-slot="{ap_slot}">"#));
    s.push_str(&game_log_feed(ap_slot, rows, 1, game_log_pages(total, GAME_LOG_RPP), GAME_LOG_RPP, ""));
    s.push_str("</div></div>");
    s
}

/// The Universal Tracker tab panel, from the slot's cached in-logic result.
pub(super) fn inlogic_panel(inlogic: Option<&crate::db::InLogic>) -> String {
    let mut s =
        String::from(r#"<div class="gtabpanel" id="tab-ut" hidden><h2>Available now</h2>"#);
    match inlogic {
        None => s.push_str(
            r#"<p class="empty">Universal Tracker hasn't mapped this slot yet — it recomputes a few seconds after the slot's items or checks change. Check back shortly.</p>"#,
        ),
        Some(il) => {
            let locs: Vec<String> = serde_json::from_str(&il.locations).unwrap_or_default();
            if locs.is_empty() && il.err.is_some() {
                let msg = match il.err.as_deref() {
                    Some("disabled") => "Universal Tracker is turned off for this game by its developers, so live availability tracking isn't available here.",
                    _ => "Couldn't compute available locations for this slot right now — it'll retry automatically.",
                };
                s.push_str(&format!(r#"<p class="empty">{msg}</p>"#));
            } else {
                let n = locs.len();
                s.push_str(&format!(
                    r#"<p class="utmeta"><b>{n}</b> location{} available right now · as of <span class="ago" data-ts="{}"></span></p>"#,
                    if n == 1 { "" } else { "s" },
                    il.ts,
                ));
                if locs.is_empty() {
                    s.push_str(r#"<p class="empty">Nothing new is available right now.</p>"#);
                } else {
                    s.push_str(r#"<div class="utlist">"#);
                    for l in &locs {
                        s.push_str(&format!(r#"<div class="utloc">{}</div>"#, esc(l)));
                    }
                    s.push_str("</div>");
                }
                s.push_str(r#"<p class="utnote">Computed by Universal Tracker from this slot's current items. Randomized options (entrance randomizer / weighted choices) can make it differ slightly from the server.</p>"#);
            }
        }
    }
    s.push_str("</div>");
    s
}

pub fn game_page(
    ap_slot: i64,
    game: &str,
    snap: &BoardSnapshot,
    slots: &[SlotRow],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    item_names: &[String],
    me: &str,
    is_admin: bool,
    cost: i64,
    points: Option<i64>,
    credits: i64,
    can_hint_common: bool,
    mc_username: Option<&str>,
    mc_addr: Option<&str>,
    files: &[crate::generate::OutputFile],
    inlogic: Option<&crate::db::InLogic>,
    log: &[crate::db::ActivityRow],
    log_total: i64,
    pt_has_pack: bool,
    pt_can_manage: bool,
) -> Html<String> {
    let live = snap.ap.get(&ap_slot).cloned().unwrap_or_default();
    let back = format!("/game/{ap_slot}");
    let here: Vec<&SlotRow> = slots.iter().filter(|r| r.ap_slot == ap_slot).collect();
    let is_common = here.iter().any(|r| r.is_common);
    let mut out = page_head(Some(me));
    out.push_str(GAME_STYLE);
    out.push_str(TABS_CSS);
    // The UT and Log tab panels are the same for the common and regular slot
    // branches; build them once.
    let ut_html = inlogic_panel(inlogic);
    let log_html = log_panel(ap_slot, log, log_total);
    // "available now" count from the slot's Universal Tracker result (if any).
    let avail = inlogic
        .filter(|il| il.err.is_none())
        .and_then(|il| serde_json::from_str::<Vec<String>>(&il.locations).ok())
        .map(|v| format!(" · {} available", v.len()))
        .unwrap_or_default();
    out.push_str(&format!(
        r#"<h1>{}</h1><div class="crumb"><a href="board">← board</a>{}</div>
<div class="meta">{} locations · sent {} · received {} · {} left{avail}</div>"#,
        esc(game),
        if is_admin { " · admin" } else { "" },
        live.world_size,
        live.sent,
        live.received,
        live.still_to_find,
    ));

    // Admin: mark this slot finished (registers its goal → collects/releases its
    // items per server settings). Hidden once goaled; /release stays separate.
    if is_admin {
        if live.status == 30 {
            out.push_str(r#"<div class="adminfin"><span class="finnote">✓ Marked finished (goaled).</span></div>"#);
        } else {
            out.push_str(&format!(
                r#"<form class="adminfin" method="post" action="game/{ap_slot}/finish" onsubmit="return confirm('Mark this game as finished? It registers their goal (announces it) and collects/releases their items per the server settings. This can\'t be undone from here.')">
<input type="hidden" name="back" value="{back}">
<button type="submit" class="finish">Mark finished</button></form>"#
            ));
        }
    }

    // The common game: shared by everyone, no per-person claim. Show how to join
    // the Minecraft server instead of claim/hint controls.
    if is_common {
        out.push_str(r#"<h2>Common game</h2>"#);
        out.push_str(r#"<p class="empty">A shared Minecraft world, open to everyone this season — drop in when you're softlocked, taking a break, or done with your own game. Its checks feed the common pool, so playing here releases items to the rest of the group.</p>"#);
        // How to join: the client must match the server exactly, or
        // NeoForge refuses the connection. Address comes last as the final step.
        let addr_step = match mc_addr {
            Some(addr) => format!(
                r#"Set your Minecraft username below so you're whitelisted, then open <b>Multiplayer</b> and connect to <b>{}</b>."#,
                esc(addr)
            ),
            None => "Open <b>Multiplayer</b> and connect — ask an admin for the server address.".to_string(),
        };
        // Bare address for the Prism steps (the full sentence above is addr_step).
        let addr = mc_addr.map(esc).unwrap_or_else(|| "ask an admin".to_string());
        // Just the NeoForge version (MC_LOADER is "NeoForge 26.2.0.90") for the
        // Prism "Install Loader → NeoForge → <version>" step.
        let loader_ver = MC_LOADER.strip_prefix("NeoForge ").unwrap_or(MC_LOADER);
        out.push_str(&format!(
            r#"<h3>How to join</h3>
<ol class="mcjoin">
<li>Get <b>Minecraft: Java Edition {ver}</b> on a genuine Mojang/Microsoft account (the server runs online-mode).</li>
<li>Install <b>{loader}</b> — <a href="game/mc/neoforge">download the installer</a>, run it, and it creates a launch profile set to the matching Minecraft version.</li>
<li>Drop the mod <code>{mod_jar}</code> — <a href="game/mc/mod">download</a> — into that profile's <code>mods/</code> folder. It's the <b>exact</b> jar the server runs; NeoForge drops you on any version mismatch.</li>
<li>{addr_step}</li>
</ol>
<p class="statline">The official Minecraft launcher and Prism both fetch the right Java for you; only a bare manual setup needs you to install <b>Java 25</b> yourself.</p>
<details class="prism"><summary>Step-by-step with Prism Launcher (recommended)</summary>
<ol class="mcjoin">
<li>Install <b>Prism Launcher</b> from <b>prismlauncher.org</b> and open it.</li>
<li>Top-right accounts button → <b>Manage Accounts</b> → <b>Add Microsoft</b> and sign in (the server is online-mode, so a genuine account is required).</li>
<li><b>Add Instance</b> → pick Minecraft <b>{ver}</b>, then in the <b>Mod Loader</b> section below the version list choose <b>NeoForge</b> → <b>{loader_ver}</b> → create. Prism downloads the matching Java (25) itself — accept if it offers. <i>(Already made the instance? Instead: <b>Edit → Version → Install Loader</b> in the right panel → NeoForge → {loader_ver}.)</i></li>
<li>Select the instance → <b>Edit</b> → <b>Mods</b> → <b>Add file…</b> and choose <code>{mod_jar}</code> (grab it from the <a href="game/mc/mod">download</a> above). Nothing else goes in there.</li>
<li><b>Launch</b> the instance → <b>Multiplayer</b> → <b>Add Server</b> → address <b>{addr}</b> → join.</li>
</ol></details>"#,
            ver = MC_GAME_VERSION,
            loader = MC_LOADER,
            loader_ver = loader_ver,
            mod_jar = crate::generate::MC_MOD_JAR,
            addr = addr,
        ));

        // Whitelist self-service: the server is whitelisted, so a player with a
        // claimed game this season registers the exact Minecraft username they'll
        // join on; the host-side `mc-whitelist-sync` applies it to the server
        // itself. Same "must own a slot" gate as hinting.
        out.push_str(r#"<h3>Your Minecraft username</h3>"#);
        if can_hint_common {
            let cur = mc_username.unwrap_or("");
            let status = if cur.is_empty() {
                String::new()
            } else {
                format!(r#"<p class="statline">Whitelisted as <b>{}</b>.</p>"#, esc(cur))
            };
            let btn = if cur.is_empty() { "Whitelist me" } else { "Update" };
            out.push_str(&format!(
                r#"<p class="empty">The server is whitelisted. Set the exact username of the Mojang/Microsoft account you'll join on — you'll be let in within a minute or so.</p>
{status}<form method="post" action="mc/username" class="hintform">
<input type="hidden" name="back" value="{back}">
<input name="mc_username" value="{cur}" placeholder="YourMinecraftName" pattern="[A-Za-z0-9_]{{3,16}}" maxlength="16" autocomplete="off" required>
<button type="submit">{btn}</button></form>"#,
                status = status,
                back = esc(&back),
                cur = esc(cur),
                btn = btn,
            ));
        } else {
            out.push_str(
                r#"<p class="empty">The server is whitelisted. <a href="claim">Claim a game</a> this season, then come back here to register your Minecraft username and get let in.</p>"#,
            );
        }

        // Tabs: everything hint-related goes in the Hints panel; the Universal
        // Tracker panel (built above) sits beside it. The join/whitelist info
        // above stays outside the tabs.
        out.push_str(tabs_open());

        // Communal hinting: only players with a claimed game this season may spend
        // the common game's OWN (shared) AP hint points — no personal/bonus points.
        if can_hint_common {
            if let Some(sid) = here.first().map(|r| r.id) {
                let items_json =
                    serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
                let have = match points {
                    Some(p) => format!(
                        r#"The common game has <b id="hintpts">{p}</b> shared hint points. "#
                    ),
                    None => String::new(),
                };
                out.push_str(&format!(
                    r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points. Any player with a game this season can spend them — they're the common game's own, earned as the group checks its locations. Search the pool and hint one.</p>
<form method="post" action="hint" class="hintform" id="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search the common game's items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<div id="hintstatus" class="inlinemsg"></div>"#
                ));
                out.push_str("<script>\nvar ITEMS=");
                out.push_str(&items_json);
                out.push_str(";\n");
                out.push_str(HINT_JS);
                out.push_str("\n</script>");
            }
        } else {
            out.push_str(
                r#"<h2>Send a hint</h2><p class="empty">Hinting the common game is open to anyone with a claimed game this season — its shared hint points are reserved for players with a slot of their own. Claim a game on the board to join in.</p>"#,
            );
        }

        // Where the common game's items are. Admins get the prioritize toggles.
        out.push_str(
            r#"<h2>Hints for this game's items</h2><p class="empty" style="margin-top:-4px">Where the common game's items are — in its own or others' worlds.</p><div id="worldhints">"#,
        );
        out.push_str(&hint_rows(
            &live.hint_views,
            &live.found_items,
            prios,
            &back,
            is_admin,
        ));
        out.push_str("</div>"); // close #worldhints
        out.push_str("</div>"); // close #tab-hints
        out.push_str(&ut_html);
        out.push_str(&poptracker_panel(ap_slot, pt_has_pack, pt_can_manage));
        out.push_str(&log_html);
        out.push_str(TABS_JS);
        out.push_str(GAME_TAB_JS);
        out.push_str("</main></body></html>");
        return Html(out);
    }

    // Players (claim / unclaim)
    out.push_str(r#"<h2>Players</h2><div class="grid">"#);
    for r in &here {
        let mine = r.claimed_by.as_deref() == Some(me);
        let (by, action) = match &r.claimed_by {
            Some(_) if mine => (
                r#"<span class="by mine">you</span>"#.to_string(),
                claim_button(r.id, "unclaim", "Unclaim", true, &back),
            ),
            Some(who) => (
                format!(r#"<span class="by">{}</span>"#, esc(who)),
                if is_admin {
                    claim_button(r.id, "unclaim", "Force unclaim", true, &back)
                } else {
                    String::new()
                },
            ),
            None => (
                r#"<span class="by free">unclaimed</span>"#.to_string(),
                claim_button(r.id, "claim", "Claim", false, &back),
            ),
        };
        out.push_str(&format!(
            r#"<div class="slot idle"><div class="prow"><span class="who">{}</span><span>{by} {action}</span></div></div>"#,
            esc(&r.name)
        ));
    }
    out.push_str("</div>");

    // Generated files for this slot — shown only to a claimant of it (or admin);
    // the caller passes an empty list to everyone else. Downloaded through the
    // claimant-gated /game/{ap_slot}/output route (not the admin one).
    if !files.is_empty() {
        out.push_str(r#"<h2>Your game files</h2>"#);
        out.push_str(r#"<p class="empty">The generator produced these for your slot — download and use them with your client to play this seed.</p>"#);
        out.push_str(r#"<div class="grid">"#);
        for f in files {
            out.push_str(&format!(
                r#"<div class="slot idle"><div class="prow"><span class="who"><code>.{}</code> file · {}</span><a class="ghost" href="game/{ap_slot}/output?file={}">download</a></div></div>"#,
                esc(&f.ext),
                human_size(f.size),
                urlq(&f.member),
            ));
        }
        out.push_str("</div>");
    }

    // Tabs: the hint form + per-item hint list live in the Hints panel; the
    // Universal Tracker panel (built above) sits beside it. Players/files above
    // stay outside the tabs.
    out.push_str(tabs_open());

    // Hinting — shown only to a player who claims a seat on this slot. Admins do
    // not get it here; they hint via the server console instead.
    let hint_slot = here
        .iter()
        .find(|r| r.claimed_by.as_deref() == Some(me))
        .map(|r| r.id);
    if let Some(sid) = hint_slot {
        let items_json = serde_json::to_string(item_names).unwrap_or_else(|_| "[]".into());
        let have = match points {
            Some(p) => format!(r#"You have <b id="hintpts">{p}</b> hint points. "#),
            None => String::new(),
        };
        let bonus = if credits > 0 {
            format!(
                r#" You also have <b id="bonushints">{credits}</b> bonus hint(s) — spent automatically when you're out of points."#
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<h2>Send a hint</h2>
<p class="empty">{have}Each hint in this world costs <b>{cost}</b> points.{bonus} Search your pool and hint one.</p>
<p class="empty"><a href="minesweeper?back={back}">Play minesweeper to earn bonus hints →</a></p>
<form method="post" action="hint" class="hintform" id="hintform">
<input type="hidden" name="slot_id" value="{sid}"><input type="hidden" name="back" value="{back}">
<input id="hintq" name="item" placeholder="search your items…" autocomplete="off" required>
<button type="submit">Hint</button></form>
<div id="hintmatches" class="matches"></div>
<div id="hintstatus" class="inlinemsg"></div>"#
        ));
        out.push_str("<script>\nvar ITEMS=");
        out.push_str(&items_json);
        out.push_str(";\n");
        out.push_str(HINT_JS);
        out.push_str("\n</script>");
    }

    // Hints for this game's items — where each of this slot's items is located.
    // Only a claimant of this game (or an admin) gets the prioritize toggles.
    // Wrapped in #worldhints so a successful hint can live-refresh it.
    let can_prioritize = hint_slot.is_some() || is_admin;
    out.push_str(
        r#"<h2>Hints for this game's items</h2><p class="empty" style="margin-top:-4px">Where this slot's items are — in its own or others' worlds.</p><div id="worldhints">"#,
    );
    out.push_str(&hint_rows(
        &live.hint_views,
        &live.found_items,
        prios,
        &back,
        can_prioritize,
    ));
    out.push_str("</div>"); // close #worldhints
    out.push_str("</div>"); // close #tab-hints
    out.push_str(&ut_html);
    out.push_str(&poptracker_panel(ap_slot, pt_has_pack, pt_can_manage));
    out.push_str(&log_html);
    out.push_str(TABS_JS);
    out.push_str(GAME_TAB_JS);

    out.push_str("</main></body></html>");
    Html(out)
}

/// The rows of the per-game hint list ("where this slot's items are"), also
/// served as a fragment for live refresh after a hint.
pub fn hint_rows(
    views: &[crate::ap::HintView],
    found_items: &[crate::ap::HintView],
    prios: &std::collections::HashSet<(i64, i64, i64)>,
    back: &str,
    can_prioritize: bool,
) -> String {
    // Active list = hints whose item is not yet found (these carry the
    // prioritize toggle). The Found section = every item the slot has received
    // (`found_items`), rendered like a found hint; it's a superset of the found
    // hints. Before the savefile has been read `found_items` is empty, so fall
    // back to the found hints from the live tracker.
    let mut active: Vec<&crate::ap::HintView> = views.iter().filter(|h| !h.found).collect();
    let mut found: Vec<&crate::ap::HintView> = if found_items.is_empty() {
        views.iter().filter(|h| h.found).collect()
    } else {
        found_items.iter().collect()
    };
    if active.is_empty() && found.is_empty() {
        return r#"<div class="empty">No hints here yet.</div>"#.to_string();
    }

    // Sort by item name (the `text` starts with it), case-insensitively.
    let name_key = |h: &&crate::ap::HintView| {
        h.text
            .split_once(" @ ")
            .map_or(h.text.as_str(), |(item, _)| item)
            .to_ascii_lowercase()
    };
    let is_prio = |h: &&crate::ap::HintView| prios.contains(&(h.finding, h.location, h.item));
    // Active: prioritized first (false < true, so negate), then alphabetical.
    active.sort_by_key(|h| (!is_prio(h), name_key(h)));
    found.sort_by_key(name_key);

    let mut out = String::new();
    if active.is_empty() {
        out.push_str(r#"<div class="empty">No unfound hints.</div>"#);
    }
    for h in &active {
        let on = prios.contains(&(h.finding, h.location, h.item));
        if !can_prioritize {
            // Non-claimants see the hints but can't reprioritize them; a
            // prioritized one is still marked.
            let star = if on { " ★" } else { "" };
            out.push_str(&format!(
                r#"<div class="hintrow"><span>{}{star}</span></div>"#,
                esc(&h.text)
            ));
            continue;
        }
        let (cls, label) = if on {
            ("", "★ prioritized")
        } else {
            ("ghost", "☆ prioritize")
        };
        out.push_str(&format!(
            r#"<div class="hintrow"><span>{}</span><form method="post" action="prioritize" style="display:inline">
<input type="hidden" name="finding" value="{}"><input type="hidden" name="location" value="{}">
<input type="hidden" name="item" value="{}"><input type="hidden" name="receiving" value="{}">
<input type="hidden" name="back" value="{}">
<button class="{cls}" type="submit">{label}</button></form></div>"#,
            esc(&h.text),
            h.finding,
            h.location,
            h.item,
            h.receiving,
            esc(back),
        ));
    }
    if !found.is_empty() {
        out.push_str(&format!(
            r#"<details class="foundhints"><summary>Found ({})</summary>"#,
            found.len()
        ));
        for h in &found {
            out.push_str(&format!(
                r#"<div class="hintrow found"><span>{}</span></div>"#,
                esc(&h.text)
            ));
        }
        out.push_str("</details>");
    }
    out
}
